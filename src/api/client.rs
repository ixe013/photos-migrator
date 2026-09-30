use std::fs;
use std::path::Path;
use std::time::Duration;
use reqwest::{header, Client, StatusCode};
use tokio::time::sleep;
use tracing::warn;

use super::auth::AuthManager;
use super::models::*;

pub const API_BASE_URL: &str = "https://photoslibrary.googleapis.com/v1";

pub struct GooglePhotosClient {
    client: Client,
    auth_mgr: AuthManager,
    account_name: String,
}

impl GooglePhotosClient {
    pub fn new(account_name: &str) -> Result<Self, String> {
        let auth_mgr = AuthManager::new()?;
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            client,
            auth_mgr,
            account_name: account_name.to_string(),
        })
    }

    async fn get_token(&self) -> Result<String, String> {
        self.auth_mgr
            .get_valid_access_token(&self.account_name, &self.client)
            .await
    }

    /// Uploads raw media bytes to Google Photos upload endpoint and returns uploadToken
    pub async fn upload_media_bytes(&self, file_path: &Path, file_name: &str) -> Result<String, String> {
        let file_bytes = fs::read(file_path)
            .map_err(|e| format!("Failed to read file for upload {:?}: {}", file_path, e))?;

        let url = format!("{}/uploads", API_BASE_URL);

        let mut attempts = 0;
        let mut backoff = Duration::from_millis(500);

        loop {
            attempts += 1;
            let token = self.get_token().await?;

            let res = self.client
                .post(&url)
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .header("Content-type", "application/octet-stream")
                .header("X-Goog-Upload-File-Name", file_name)
                .header("X-Goog-Upload-Protocol", "raw")
                .body(file_bytes.clone())
                .send()
                .await;

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let upload_token = resp
                            .text()
                            .await
                            .map_err(|e| format!("Failed to read uploadToken: {}", e))?;
                        return Ok(upload_token.trim().to_string());
                    } else if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
                        if attempts >= 5 {
                            return Err(format!("Upload rate limit/unavailable exceeded max retries: {}", status));
                        }
                        warn!("Rate limit or server busy ({}). Backing off for {:?}...", status, backoff);
                        sleep(backoff).await;
                        backoff *= 2;
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        return Err(format!("Upload failed with status {}: {}", status, text));
                    }
                }
                Err(e) => {
                    if attempts >= 5 {
                        return Err(format!("Upload connection error: {}", e));
                    }
                    warn!("Connection error during upload ({}). Retrying in {:?}...", e, backoff);
                    sleep(backoff).await;
                    backoff *= 2;
                }
            }
        }
    }

    /// Creates Google Photos media items from upload tokens (batch of up to 50 items)
    pub async fn batch_create_media_items(
        &self,
        album_id: Option<&str>,
        items: Vec<NewMediaItem>,
    ) -> Result<Vec<(String, String)>, String> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/mediaItems:batchCreate", API_BASE_URL);
        let request_body = BatchCreateMediaItemsRequest {
            album_id: album_id.map(|s| s.to_string()),
            new_media_items: items,
        };

        let mut attempts = 0;
        let mut backoff = Duration::from_millis(1000);

        loop {
            attempts += 1;
            let token = self.get_token().await?;

            let res = self.client
                .post(&url)
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .json(&request_body)
                .send()
                .await;

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let create_resp: BatchCreateMediaItemsResponse = resp
                            .json()
                            .await
                            .map_err(|e| format!("Failed to parse batchCreate response: {}", e))?;

                        let mut results = Vec::new();
                        if let Some(res_list) = create_resp.new_media_item_results {
                            for item_res in res_list {
                                if let (Some(token), Some(media)) = (item_res.upload_token, item_res.media_item) {
                                    results.push((token, media.id));
                                } else if let Some(status) = item_res.status.message {
                                    warn!("Item batchCreate returned error status: {}", status);
                                }
                            }
                        }
                        return Ok(results);
                    } else if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
                        if attempts >= 5 {
                            return Err(format!("batchCreate rate limit exceeded: {}", status));
                        }
                        warn!("batchCreate rate limit / busy. Backing off for {:?}...", backoff);
                        sleep(backoff).await;
                        backoff *= 2;
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        return Err(format!("batchCreate failed with status {}: {}", status, text));
                    }
                }
                Err(e) => {
                    if attempts >= 5 {
                        return Err(format!("batchCreate connection error: {}", e));
                    }
                    sleep(backoff).await;
                    backoff *= 2;
                }
            }
        }
    }

    /// Creates an album on the target account
    pub async fn create_album(&self, title: &str) -> Result<Album, String> {
        let url = format!("{}/albums", API_BASE_URL);
        let request_body = CreateAlbumRequest {
            album: AlbumTitle {
                title: title.to_string(),
            },
        };

        let token = self.get_token().await?;
        let res = self.client
            .post(&url)
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Album create request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Failed to create album '{}': {}", title, text));
        }

        let album: Album = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse album response: {}", e))?;

        Ok(album)
    }

    /// Enables sharing on an album and returns its shareable URL
    pub async fn share_album(&self, album_id: &str) -> Result<ShareInfo, String> {
        let url = format!("{}/albums/{}:share", API_BASE_URL, album_id);
        let request_body = ShareAlbumRequest {
            shared_album_options: SharedAlbumOptions {
                is_collaborative: true,
                is_commentable: true,
            },
        };

        let token = self.get_token().await?;
        let res = self.client
            .post(&url)
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Share album request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Failed to share album ID '{}': {}", album_id, text));
        }

        let album: Album = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse share response: {}", e))?;

        album
            .share_info
            .ok_or_else(|| "Album shared but no shareInfo returned by Google".to_string())
    }

    /// Adds existing media items to an album (e.g. for photos belonging to multiple albums)
    pub async fn add_media_to_album(&self, album_id: &str, media_item_ids: &[String]) -> Result<(), String> {
        if media_item_ids.is_empty() {
            return Ok(());
        }

        for chunk in media_item_ids.chunks(50) {
            let url = format!("{}/albums/{}:batchAddMediaItems", API_BASE_URL, album_id);
            let request_body = BatchAddMediaItemsRequest {
                media_item_ids: chunk.to_vec(),
            };

            let token = self.get_token().await?;
            let res = self.client
                .post(&url)
                .header(header::AUTHORIZATION, format!("Bearer {}", token))
                .json(&request_body)
                .send()
                .await
                .map_err(|e| format!("batchAddMediaItems request failed: {}", e))?;

            if !res.status().is_success() {
                let text = res.text().await.unwrap_or_default();
                warn!("Failed to add batch to album {}: {}", album_id, text);
            }
        }

        Ok(())
    }
}
