//! gRPC JSON Transcoding
//!
//! Provides JSON API endpoints that mirror gRPC services.
//! Endpoints are prefixed with `/grpc/` to distinguish from native HTTP API.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::jobs::JobPriority;
use super::http::AppState;

// ========== Request Types ==========

#[derive(Deserialize)]
pub struct EtcScrapeRequest {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub download_path: String,
    #[serde(default = "default_headless")]
    pub headless: bool,
}

#[derive(Deserialize)]
pub struct EtcAccountInfo {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Deserialize)]
pub struct EtcBatchRequest {
    #[serde(default)]
    pub accounts: Vec<EtcAccountInfo>,
    #[serde(default)]
    pub download_path: String,
    #[serde(default = "default_headless")]
    pub headless: bool,
}

#[derive(Deserialize, Default)]
pub struct EtcBatchEnvRequest {
    #[serde(default)]
    pub download_path: String,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub headless_set: bool,
}


fn default_headless() -> bool {
    true
}

fn default_download_path() -> String {
    std::env::var("ETC_DOWNLOAD_PATH").unwrap_or_else(|_| "./downloads".to_string())
}

// ========== Response Types (matching proto definitions) ==========

#[derive(Serialize)]
pub struct EtcScrapeResponse {
    pub job_id: String,
    pub status: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct GetJobResponse {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<Job>,
}

#[derive(Serialize)]
pub struct ListJobsResponse {
    pub jobs: Vec<Job>,
    pub count: i32,
}

#[derive(Serialize)]
pub struct GetQueueStatusResponse {
    pub queue_length: i32,
    pub running_jobs: i32,
    pub is_idle: bool,
}

#[derive(Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub name: String,
    pub file_count: i32,
}

#[derive(Serialize)]
pub struct ListSessionFilesResponse {
    pub session_id: String,
    pub files: Vec<FileInfo>,
}

#[derive(Serialize)]
pub struct FileInfo {
    pub name: String,
    pub size: i64,
}

#[derive(Serialize)]
pub struct HealthCheckResponse {
    pub status: String,
    pub version: String,
    pub uptime: i64,
}

#[derive(Serialize)]
pub struct Job {
    pub id: String,
    pub job_type: String,
    pub priority: String,
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub completed_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etc_result: Option<EtcScrapeResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_result: Option<EtcBatchResult>,
    #[serde(skip_serializing_if = "is_zero")]
    pub current_account_index: i32,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

#[derive(Serialize)]
pub struct EtcScrapeResult {
    pub csv_path: String,
    pub csv_size: i32,
}

#[derive(Serialize)]
pub struct EtcBatchResult {
    pub session_folder: String,
    pub accounts: Vec<AccountResult>,
    pub total_count: i32,
    pub success_count: i32,
    pub fail_count: i32,
}

#[derive(Serialize)]
pub struct AccountResult {
    pub user_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub csv_path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub csv_size: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}


// ========== Helper Functions ==========

fn get_accounts_from_env() -> Result<Vec<crate::jobs::EtcAccountInfo>, String> {
    let accounts_json = std::env::var("ETC_ACCOUNTS")
        .map_err(|_| "ETC_ACCOUNTS environment variable not set".to_string())?;

    #[derive(Deserialize)]
    struct AccountInfo {
        user_id: String,
        password: String,
        #[serde(default)]
        name: String,
    }

    let accounts: Vec<AccountInfo> = serde_json::from_str(&accounts_json)
        .map_err(|e| format!("Failed to parse ETC_ACCOUNTS: {}", e))?;

    if accounts.is_empty() {
        return Err("ETC_ACCOUNTS is empty".to_string());
    }

    Ok(accounts
        .into_iter()
        .map(|a| crate::jobs::EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect())
}

fn job_to_response(job: &crate::jobs::Job) -> Job {
    Job {
        id: job.id.clone(),
        job_type: format!("{:?}", job.job_type),
        priority: format!("{:?}", job.priority),
        status: format!("{:?}", job.status),
        created_at: job.created_at.to_rfc3339(),
        started_at: job.started_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        completed_at: job.completed_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        error: job.error.clone().unwrap_or_default(),
        etc_result: job.etc_result.as_ref().map(|r| EtcScrapeResult {
            csv_path: r.csv_path.clone(),
            csv_size: r.csv_size as i32,
        }),
        batch_result: job.batch_result.as_ref().map(|r| EtcBatchResult {
            session_folder: r.session_folder.clone(),
            accounts: r
                .accounts
                .iter()
                .map(|a| AccountResult {
                    user_id: a.user_id.clone(),
                    name: a.name.clone(),
                    status: format!("{:?}", a.status),
                    csv_path: a.csv_path.clone().unwrap_or_default(),
                    csv_size: a.csv_size.unwrap_or(0) as i32,
                    error: a.error.clone().unwrap_or_default(),
                })
                .collect(),
            total_count: r.total_count as i32,
            success_count: r.success_count as i32,
            fail_count: r.fail_count as i32,
        }),
        current_account_index: job.current_account_index.unwrap_or(0) as i32,
    }
}

// ========== Router ==========

/// Create gRPC JSON transcoding router
/// All endpoints are under /grpc/ prefix
pub fn create_grpc_json_router() -> Router<Arc<AppState>> {
    Router::new()
        // EtcScraperService
        .route("/grpc/EtcScrape", post(handle_etc_scrape))
        .route("/grpc/EtcScrapeQueue", post(handle_etc_scrape_queue))
        .route("/grpc/EtcScrapeBatch", post(handle_etc_scrape_batch))
        .route("/grpc/EtcScrapeBatchQueue", post(handle_etc_scrape_batch_queue))
        .route("/grpc/EtcScrapeBatchEnv", post(handle_etc_scrape_batch_env))
        .route("/grpc/EtcScrapeBatchEnvQueue", post(handle_etc_scrape_batch_env_queue))
        .route("/grpc/GetJob/:job_id", get(handle_get_job))
        .route("/grpc/ListJobs", get(handle_list_jobs))
        .route("/grpc/GetQueueStatus", get(handle_get_queue_status))
        .route("/grpc/ListSessions", get(handle_list_sessions))
        .route("/grpc/ListSessionFiles/:session_id", get(handle_list_session_files))
        // BrowserRenderService
        .route("/grpc/HealthCheck", get(handle_health_check))
}

// ========== Handlers ==========

async fn handle_etc_scrape(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EtcScrapeRequest>,
) -> impl IntoResponse {
    info!("gRPC-JSON EtcScrape called for user_id={}", req.user_id);

    let etc_request = crate::jobs::EtcScrapeRequest {
        user_id: req.user_id.clone(),
        password: req.password,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: req.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_job(etc_request, JobPriority::Normal)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "pending".to_string(),
        message: "ETC scrape job created.".to_string(),
    })
}

async fn handle_etc_scrape_queue(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EtcScrapeRequest>,
) -> impl IntoResponse {
    info!("gRPC-JSON EtcScrapeQueue called for user_id={}", req.user_id);

    let etc_request = crate::jobs::EtcScrapeRequest {
        user_id: req.user_id.clone(),
        password: req.password,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: req.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_job(etc_request, JobPriority::Low)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "queued".to_string(),
        message: "ETC scrape job queued.".to_string(),
    })
}

async fn handle_etc_scrape_batch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EtcBatchRequest>,
) -> impl IntoResponse {
    let account_count = req.accounts.len();
    info!("gRPC-JSON EtcScrapeBatch called for {} accounts", account_count);

    let accounts: Vec<crate::jobs::EtcAccountInfo> = req
        .accounts
        .into_iter()
        .map(|a| crate::jobs::EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect();

    let batch_request = crate::jobs::EtcBatchRequest {
        accounts,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: req.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(batch_request, JobPriority::Normal)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "pending".to_string(),
        message: format!("ETC batch scrape job created for {} accounts.", account_count),
    })
}

async fn handle_etc_scrape_batch_queue(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EtcBatchRequest>,
) -> impl IntoResponse {
    let account_count = req.accounts.len();
    info!("gRPC-JSON EtcScrapeBatchQueue called for {} accounts", account_count);

    let accounts: Vec<crate::jobs::EtcAccountInfo> = req
        .accounts
        .into_iter()
        .map(|a| crate::jobs::EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect();

    let batch_request = crate::jobs::EtcBatchRequest {
        accounts,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: req.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(batch_request, JobPriority::Low)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "queued".to_string(),
        message: format!("ETC batch scrape job queued for {} accounts.", account_count),
    })
}

async fn handle_etc_scrape_batch_env(
    State(state): State<Arc<AppState>>,
    body: Option<Json<EtcBatchEnvRequest>>,
) -> impl IntoResponse {
    let req = body.map(|b| b.0).unwrap_or_default();
    info!("gRPC-JSON EtcScrapeBatchEnv called");

    let accounts = match get_accounts_from_env() {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e})),
            )
                .into_response()
        }
    };

    let account_count = accounts.len();
    let batch_request = crate::jobs::EtcBatchRequest {
        accounts,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: if req.headless_set { req.headless } else { true },
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(batch_request, JobPriority::Normal)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "pending".to_string(),
        message: format!("ETC batch scrape job created for {} accounts from env.", account_count),
    })
    .into_response()
}

async fn handle_etc_scrape_batch_env_queue(
    State(state): State<Arc<AppState>>,
    body: Option<Json<EtcBatchEnvRequest>>,
) -> impl IntoResponse {
    let req = body.map(|b| b.0).unwrap_or_default();
    info!("gRPC-JSON EtcScrapeBatchEnvQueue called");

    let accounts = match get_accounts_from_env() {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e})),
            )
                .into_response()
        }
    };

    let account_count = accounts.len();
    let batch_request = crate::jobs::EtcBatchRequest {
        accounts,
        download_path: if req.download_path.is_empty() {
            default_download_path()
        } else {
            req.download_path
        },
        headless: if req.headless_set { req.headless } else { true },
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(batch_request, JobPriority::Low)
        .await;

    Json(EtcScrapeResponse {
        job_id,
        status: "queued".to_string(),
        message: format!("ETC batch scrape job queued for {} accounts from env.", account_count),
    })
    .into_response()
}

async fn handle_get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    info!("gRPC-JSON GetJob called for job_id={}", job_id);

    match state.job_manager.get_job(&job_id).await {
        Some(job) => Json(GetJobResponse {
            found: true,
            job: Some(job_to_response(&job)),
        }),
        None => Json(GetJobResponse {
            found: false,
            job: None,
        }),
    }
}

async fn handle_list_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("gRPC-JSON ListJobs called");

    let jobs = state.job_manager.get_all_jobs().await;
    let count = jobs.len() as i32;
    let response_jobs: Vec<Job> = jobs.iter().map(job_to_response).collect();

    Json(ListJobsResponse {
        jobs: response_jobs,
        count,
    })
}

async fn handle_get_queue_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    info!("gRPC-JSON GetQueueStatus called");

    let queue_length = state.job_manager.queue_length().await as i32;
    let running_jobs = state.job_manager.running_job_count().await as i32;
    let is_idle = state.job_manager.is_idle().await;

    Json(GetQueueStatusResponse {
        queue_length,
        running_jobs,
        is_idle,
    })
}

async fn handle_list_sessions() -> impl IntoResponse {
    info!("gRPC-JSON ListSessions called");

    let download_path = default_download_path();

    let entries = match std::fs::read_dir(&download_path) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to read directory: {}", e)})),
            )
                .into_response()
        }
    };

    let mut sessions: Vec<SessionInfo> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.len() == 15 && name.chars().nth(8) == Some('_') && entry.path().is_dir() {
                let file_count = std::fs::read_dir(entry.path())
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                e.path()
                                    .extension()
                                    .map(|ext| ext.eq_ignore_ascii_case("csv"))
                                    .unwrap_or(false)
                            })
                            .count()
                    })
                    .unwrap_or(0) as i32;
                Some(SessionInfo { name, file_count })
            } else {
                None
            }
        })
        .collect();

    sessions.sort_by(|a, b| b.name.cmp(&a.name));

    Json(ListSessionsResponse { sessions }).into_response()
}

async fn handle_list_session_files(Path(session_id): Path<String>) -> impl IntoResponse {
    info!("gRPC-JSON ListSessionFiles called for session_id={}", session_id);

    if session_id.len() != 15 || session_id.chars().nth(8) != Some('_') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid session ID format"})),
        )
            .into_response();
    }

    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid session ID"})),
        )
            .into_response();
    }

    let download_path = default_download_path();
    let session_path = std::path::Path::new(&download_path).join(&session_id);

    if !session_path.is_dir() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("Session not found: {}", session_id)})),
        )
            .into_response();
    }

    let entries = match std::fs::read_dir(&session_path) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to read session: {}", e)})),
            )
                .into_response()
        }
    };

    let mut files: Vec<FileInfo> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("csv"))
                    .unwrap_or(false)
            {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
                Some(FileInfo { name, size })
            } else {
                None
            }
        })
        .collect();

    files.sort_by(|a, b| a.name.cmp(&b.name));

    Json(ListSessionFilesResponse { session_id, files }).into_response()
}

async fn handle_health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs() as i64;

    Json(HealthCheckResponse {
        status: "healthy".to_string(),
        version: "1.0.0".to_string(),
        uptime,
    })
}
