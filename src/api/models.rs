use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleClientCredentials {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokenInfo {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64, // Unix timestamp in seconds
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub token_type: String,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAlbumRequest {
    pub album: AlbumTitle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumTitle {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    #[serde(rename = "productUrl")]
    pub product_url: Option<String>,
    #[serde(rename = "shareInfo")]
    pub share_info: Option<ShareInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareAlbumRequest {
    #[serde(rename = "sharedAlbumOptions")]
    pub shared_album_options: SharedAlbumOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedAlbumOptions {
    #[serde(rename = "isCollaborative")]
    pub is_collaborative: bool,
    #[serde(rename = "isCommentable")]
    pub is_commentable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareInfo {
    #[serde(rename = "shareableUrl")]
    pub shareable_url: Option<String>,
    #[serde(rename = "shareToken")]
    pub share_token: Option<String>,
    #[serde(rename = "isJoined")]
    pub is_joined: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateMediaItemsRequest {
    #[serde(rename = "albumId", skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(rename = "newMediaItems")]
    pub new_media_items: Vec<NewMediaItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMediaItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "simpleMediaItem")]
    pub simple_media_item: SimpleMediaItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleMediaItem {
    #[serde(rename = "uploadToken")]
    pub upload_token: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCreateMediaItemsResponse {
    #[serde(rename = "newMediaItemResults")]
    pub new_media_item_results: Option<Vec<MediaItemResult>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItemResult {
    #[serde(rename = "uploadToken")]
    pub upload_token: Option<String>,
    pub status: StatusResult,
    #[serde(rename = "mediaItem")]
    pub media_item: Option<MediaItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub message: Option<String>,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub description: Option<String>,
    #[serde(rename = "productUrl")]
    pub product_url: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchAddMediaItemsRequest {
    #[serde(rename = "mediaItemIds")]
    pub media_item_ids: Vec<String>,
}
