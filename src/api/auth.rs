use std::fs;
use std::path::PathBuf;
use chrono::Utc;
use rand::{distributions::Alphanumeric, Rng};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tiny_http::{Response, Server};
use tracing::info;
use url::Url;

use super::models::{GoogleClientCredentials, StoredTokenInfo, TokenResponse};

pub const DEFAULT_REDIRECT_PORT: u16 = 8085;
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/photoslibrary",
    "https://www.googleapis.com/auth/photoslibrary.sharing",
    "https://www.googleapis.com/auth/userinfo.email",
];

pub struct AuthManager {
    config_dir: PathBuf,
}

impl AuthManager {
    pub fn new() -> Result<Self, String> {
        let base_dirs = directories::BaseDirs::new()
            .ok_or_else(|| "Could not determine user home directory".to_string())?;
        let config_dir = base_dirs.config_dir().join("photos-migrator");
        fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config directory at {:?}: {}", config_dir, e))?;
        Ok(Self { config_dir })
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.config_dir.join("credentials.json")
    }

    pub fn token_path(&self, account_name: &str) -> PathBuf {
        self.config_dir.join(format!("token_{}.json", account_name))
    }

    pub fn save_client_credentials(&self, creds: &GoogleClientCredentials) -> Result<(), String> {
        let json = serde_json::to_string_pretty(creds)
            .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
        fs::write(self.credentials_path(), json)
            .map_err(|e| format!("Failed to save credentials: {}", e))?;
        Ok(())
    }

    pub fn load_client_credentials(&self) -> Result<Option<GoogleClientCredentials>, String> {
        let path = self.credentials_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read credentials from {:?}: {}", path, e))?;
        
        // Handle both direct struct and Google's standard downloaded client_secret_*.json format
        if let Ok(creds) = serde_json::from_str::<GoogleClientCredentials>(&content) {
            return Ok(Some(creds));
        }

        // Try standard Google console json: {"installed": {"client_id": "...", "client_secret": "..."}}
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            let installed = val.get("installed").or_else(|| val.get("web"));
            if let Some(inner) = installed {
                if let (Some(cid), Some(sec)) = (
                    inner.get("client_id").and_then(|v| v.as_str()),
                    inner.get("client_secret").and_then(|v| v.as_str()),
                ) {
                    return Ok(Some(GoogleClientCredentials {
                        client_id: cid.to_string(),
                        client_secret: sec.to_string(),
                    }));
                }
            }
        }

        Err("Failed to parse Google credentials file".to_string())
    }

    pub fn save_token(&self, account_name: &str, token: &StoredTokenInfo) -> Result<(), String> {
        let json = serde_json::to_string_pretty(token)
            .map_err(|e| format!("Failed to serialize token: {}", e))?;
        fs::write(self.token_path(account_name), json)
            .map_err(|e| format!("Failed to write token file: {}", e))?;
        Ok(())
    }

    pub fn load_token(&self, account_name: &str) -> Result<Option<StoredTokenInfo>, String> {
        let path = self.token_path(account_name);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read token file: {}", e))?;
        let token = serde_json::from_str::<StoredTokenInfo>(&content)
            .map_err(|e| format!("Failed to parse token file: {}", e))?;
        Ok(Some(token))
    }

    pub async fn get_valid_access_token(
        &self,
        account_name: &str,
        client: &Client,
    ) -> Result<String, String> {
        let creds = self
            .load_client_credentials()?
            .ok_or_else(|| "Google Cloud credentials not found. Run `photos-migrator auth` first.".to_string())?;

        let mut token = self
            .load_token(account_name)?
            .ok_or_else(|| format!("Account '{}' not authenticated. Run `photos-migrator auth` first.", account_name))?;

        let now = Utc::now().timestamp();
        // Refresh token if within 60 seconds of expiry
        if token.expires_at <= now + 60 {
            if let Some(refresh_token) = &token.refresh_token {
                info!("Refreshing access token for account '{}'...", account_name);
                let new_token = Self::refresh_access_token(client, &creds, refresh_token).await?;
                token.access_token = new_token.access_token.clone();
                token.expires_at = Utc::now().timestamp() + new_token.expires_in;
                if new_token.refresh_token.is_some() {
                    token.refresh_token = new_token.refresh_token;
                }
                self.save_token(account_name, &token)?;
            } else {
                return Err(format!("Token for '{}' expired and no refresh token is stored. Please re-authenticate.", account_name));
            }
        }

        Ok(token.access_token)
    }

    pub async fn login_interactive(
        &self,
        account_name: &str,
        creds: &GoogleClientCredentials,
    ) -> Result<(), String> {
        let redirect_uri = format!("http://127.0.0.1:{}/oauth/callback", DEFAULT_REDIRECT_PORT);
        let code_verifier: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(64)
            .map(char::from)
            .collect();

        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let hash = hasher.finalize();
        let code_challenge = base64_url_encode(&hash);

        let mut auth_url = Url::parse(AUTH_URL).unwrap();
        auth_url.query_pairs_mut()
            .append_pair("client_id", &creds.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");

        println!("\n========================================================");
        println!(" Authenticating account: [{}]", account_name);
        println!("========================================================");
        println!("Opening your web browser to authorize Google Photos access...\n");
        println!("If the browser does not open automatically, visit this URL:\n{}\n", auth_url);

        let _ = open::that(auth_url.as_str());

        let server = Server::http(format!("127.0.0.1:{}", DEFAULT_REDIRECT_PORT))
            .map_err(|e| format!("Failed to start local OAuth listener on port {}: {}", DEFAULT_REDIRECT_PORT, e))?;

        println!("Waiting for authorization from browser...");

        let mut auth_code = None;
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            if let Ok(req_url) = Url::parse(&format!("http://127.0.0.1:{}{}", DEFAULT_REDIRECT_PORT, url)) {
                if req_url.path() == "/oauth/callback" {
                    let pairs = req_url.query_pairs();
                    for (k, v) in pairs {
                        if k == "code" {
                            auth_code = Some(v.to_string());
                            break;
                        }
                    }

                    let html = if auth_code.is_some() {
                        "<html><body style='font-family:sans-serif; text-align:center; padding-top:50px;'>\
                         <h1 style='color:#1a73e8;'>Authentication Successful!</h1>\
                         <p>You can close this tab and return to the terminal.</p></body></html>"
                    } else {
                        "<html><body style='font-family:sans-serif; text-align:center; padding-top:50px;'>\
                         <h1 style='color:#d93025;'>Authentication Failed</h1>\
                         <p>No authorization code was found in the callback.</p></body></html>"
                    };

                    let response = Response::from_string(html)
                        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap());
                    let _ = request.respond(response);
                    break;
                }
            }
        }

        let code = auth_code.ok_or_else(|| "Did not receive authorization code from Google".to_string())?;
        println!("Received authorization code. Exchanging for access tokens...");

        let client = Client::new();
        let token_resp = Self::exchange_code_for_token(&client, creds, &code, &code_verifier, &redirect_uri).await?;

        let expires_at = Utc::now().timestamp() + token_resp.expires_in;
        let stored_info = StoredTokenInfo {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token,
            expires_at,
            scope: token_resp.scope,
        };

        self.save_client_credentials(creds)?;
        self.save_token(account_name, &stored_info)?;

        println!("Successfully authenticated account [{}]!\n", account_name);
        Ok(())
    }

    async fn exchange_code_for_token(
        client: &Client,
        creds: &GoogleClientCredentials,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenResponse, String> {
        let params = [
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("code", code),
            ("code_verifier", code_verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ];

        let res = client
            .post(TOKEN_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("Token request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Google token exchange error: {}", text));
        }

        let token_resp: TokenResponse = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

        Ok(token_resp)
    }

    async fn refresh_access_token(
        client: &Client,
        creds: &GoogleClientCredentials,
        refresh_token: &str,
    ) -> Result<TokenResponse, String> {
        let params = [
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let res = client
            .post(TOKEN_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("Refresh request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Google token refresh error: {}", text));
        }

        let token_resp: TokenResponse = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse refresh response: {}", e))?;

        Ok(token_resp)
    }
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(data)
}
