use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::config::Config;
use crate::storage::Storage;

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
    #[error("Session error: {0}")]
    SessionError(String),
}

impl RendererError {
    /// Get the appropriate HTTP status code for this error
    pub fn http_status_code(&self) -> u16 {
        match self {
            Self::SessionError(_) => 401,
            Self::Storage(_) => 500,
        }
    }
}

pub type Result<T> = std::result::Result<T, RendererError>;

/// Response from Hono API (gRPC response)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HonoApiResponse {
    pub success: bool,
    pub records_added: i32,
    pub total_records: i32,
    pub message: String,
}

/// Session manager for vehicle data operations
///
/// Note: Vehicle data scraping is now handled by DtakologScraper in rust-scraper.
/// This struct only manages session state in storage.
pub struct Renderer {
    #[allow(dead_code)]
    config: Arc<Config>,
    storage: Arc<Storage>,
}

impl Renderer {
    /// Create a new renderer instance (session manager)
    pub async fn new(config: Arc<Config>, storage: Arc<Storage>) -> Result<Self> {
        info!("Initializing session manager...");
        Ok(Renderer { config, storage })
    }

    /// Check if a session is valid
    pub async fn check_session(&self, session_id: &str) -> (bool, String) {
        match self.storage.get_session(session_id).await {
            Ok(Some(session)) => {
                if session.expires_at > Utc::now() {
                    (true, "Session is valid".to_string())
                } else {
                    (false, "Session expired".to_string())
                }
            }
            Ok(None) => (false, "Session not found".to_string()),
            Err(e) => (false, format!("Error checking session: {}", e)),
        }
    }

    /// Clear a session
    pub async fn clear_session(&self, session_id: &str) -> Result<()> {
        self.storage
            .delete_session(session_id)
            .await
            .map_err(RendererError::Storage)
    }
}
