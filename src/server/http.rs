use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Path, Query, State},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

use crate::browser::Renderer;
use crate::config::Config;
use crate::jobs::{EtcAccountInfo, EtcBatchRequest, EtcScrapeRequest, JobManager, JobPriority};
use crate::storage::Storage;

use super::grpc_json::create_grpc_json_router;

/// Shared application state
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<Storage>,
    pub renderer: Arc<Renderer>,
    pub job_manager: Arc<JobManager>,
    pub start_time: Instant,
}

/// Create HTTP router with all endpoints
pub fn create_router(state: Arc<AppState>) -> Router {
    // Configure CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    // Create gRPC JSON transcoding router
    let grpc_json_router = create_grpc_json_router();

    Router::new()
        // API endpoints
        .route("/v1/vehicle/data", get(handle_vehicle_data))
        .route("/v1/etc/scrape", post(handle_etc_scrape))
        .route("/v1/etc/scrape/queue", post(handle_etc_scrape_queue))
        .route("/v1/etc/scrape/batch", post(handle_etc_scrape_batch))
        .route("/v1/etc/scrape/batch/queue", post(handle_etc_scrape_batch_queue))
        .route("/v1/etc/scrape/batch/env", post(handle_etc_scrape_batch_env))
        .route("/v1/etc/scrape/batch/env/queue", post(handle_etc_scrape_batch_env_queue))
        .route("/v1/etc/sessions", get(handle_etc_sessions))
        .route("/v1/etc/sessions/:session_id/files", get(handle_etc_session_files))
        .route("/v1/etc/sessions/:session_id/files/:filename", get(handle_etc_file_download))
        .route("/v1/job/:id", get(handle_job_status))
        .route("/v1/jobs", get(handle_jobs_list))
        .route("/v1/jobs/queue", get(handle_queue_status))
        .route("/v1/session/check", get(handle_session_check))
        .route("/v1/session/clear", delete(handle_session_clear))
        // Health and metrics
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        // gRPC JSON transcoding endpoints (stateless, merged directly)
        .merge(grpc_json_router)
        .layer(cors)
        .with_state(state)
}

// ========== Response Types ==========

#[derive(Serialize)]
struct VehicleDataResponse {
    job_id: String,
    status: String,
    message: String,
}

#[derive(Serialize)]
struct JobsListResponse {
    jobs: Vec<crate::jobs::Job>,
    count: usize,
}

#[derive(Serialize)]
struct QueueStatusResponse {
    queue_length: usize,
    running_jobs: usize,
    is_idle: bool,
}

#[derive(Deserialize)]
struct EtcScrapeRequestBody {
    user_id: String,
    password: String,
    #[serde(default = "default_download_path")]
    download_path: String,
    #[serde(default = "default_headless")]
    headless: bool,
}

fn default_download_path() -> String {
    std::env::var("ETC_DOWNLOAD_PATH").unwrap_or_else(|_| "./downloads".to_string())
}

fn default_headless() -> bool {
    true
}

#[derive(Deserialize)]
struct EtcAccountInfoBody {
    user_id: String,
    password: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct EtcBatchRequestBody {
    accounts: Vec<EtcAccountInfoBody>,
    #[serde(default = "default_download_path")]
    download_path: String,
    #[serde(default = "default_headless")]
    headless: bool,
}

/// Request body for env-based batch scrape (optional overrides)
#[derive(Deserialize, Default)]
struct EtcBatchEnvRequestBody {
    #[serde(default)]
    download_path: Option<String>,
    #[serde(default)]
    headless: Option<bool>,
}

/// Get accounts from ETC_ACCOUNTS environment variable
/// Format: JSON array of {"user_id": "...", "password": "...", "name": "..."}
fn get_accounts_from_env() -> Result<Vec<EtcAccountInfo>, String> {
    let accounts_json = std::env::var("ETC_ACCOUNTS")
        .map_err(|_| "ETC_ACCOUNTS environment variable not set".to_string())?;

    let accounts: Vec<EtcAccountInfoBody> = serde_json::from_str(&accounts_json)
        .map_err(|e| format!("Failed to parse ETC_ACCOUNTS: {}", e))?;

    if accounts.is_empty() {
        return Err("ETC_ACCOUNTS is empty".to_string());
    }

    Ok(accounts
        .into_iter()
        .map(|a| EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect())
}

#[derive(Deserialize)]
struct SessionQuery {
    session_id: Option<String>,
}

#[derive(Serialize)]
struct SessionCheckResponse {
    is_valid: bool,
    message: String,
}

#[derive(Serialize)]
struct SessionClearResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    uptime: f64,
    env: EnvStatus,
}

#[derive(Serialize)]
struct EnvStatus {
    etc_accounts: bool,
    etc_download_path: Option<String>,
}

#[derive(Serialize)]
struct MetricsResponse {
    uptime_seconds: f64,
    timestamp: i64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ========== ETC Session/File Types ==========

#[derive(Serialize)]
struct SessionInfo {
    name: String,
    file_count: usize,
}

#[derive(Serialize)]
struct SessionsListResponse {
    sessions: Vec<SessionInfo>,
}

#[derive(Serialize)]
struct FileInfo {
    name: String,
    size: u64,
}

#[derive(Serialize)]
struct SessionFilesResponse {
    session_id: String,
    files: Vec<FileInfo>,
}

// ========== Handlers ==========

/// Vehicle data endpoint - creates a new job
async fn handle_vehicle_data(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let job_id = state.job_manager.create_job().await;
    info!("Created new job: {}", job_id);

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "pending".to_string(),
            message: "Job created successfully. Use /v1/job/{id} to check status.".to_string(),
        }),
    )
}

/// Job status endpoint
async fn handle_job_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    match state.job_manager.get_job(&job_id).await {
        Some(job) => (StatusCode::OK, Json(job)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Job not found: {}", job_id),
            }),
        )
            .into_response(),
    }
}

/// Jobs list endpoint
async fn handle_jobs_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let jobs = state.job_manager.get_all_jobs().await;
    let count = jobs.len();

    Json(JobsListResponse { jobs, count })
}

/// ETC scrape endpoint - runs immediately
async fn handle_etc_scrape(
    State(state): State<Arc<AppState>>,
    Json(body): Json<EtcScrapeRequestBody>,
) -> impl IntoResponse {
    let request = EtcScrapeRequest {
        user_id: body.user_id.clone(),
        password: body.password,
        download_path: body.download_path,
        headless: body.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_job(request, JobPriority::Normal)
        .await;

    info!("Created ETC scrape job (immediate): {} for user {}", job_id, body.user_id);

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "pending".to_string(),
            message: "ETC scrape job created. Use /v1/job/{id} to check status.".to_string(),
        }),
    )
}

/// ETC scrape queue endpoint - runs when idle
async fn handle_etc_scrape_queue(
    State(state): State<Arc<AppState>>,
    Json(body): Json<EtcScrapeRequestBody>,
) -> impl IntoResponse {
    let request = EtcScrapeRequest {
        user_id: body.user_id.clone(),
        password: body.password,
        download_path: body.download_path,
        headless: body.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_job(request, JobPriority::Low)
        .await;

    info!("Created ETC scrape job (queued): {} for user {}", job_id, body.user_id);

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "queued".to_string(),
            message: "ETC scrape job queued. Will run when system is idle. Use /v1/job/{id} to check status.".to_string(),
        }),
    )
}

/// ETC batch scrape endpoint - runs immediately
async fn handle_etc_scrape_batch(
    State(state): State<Arc<AppState>>,
    Json(body): Json<EtcBatchRequestBody>,
) -> impl IntoResponse {
    let accounts: Vec<EtcAccountInfo> = body
        .accounts
        .into_iter()
        .map(|a| EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect();

    let account_count = accounts.len();
    let request = EtcBatchRequest {
        accounts,
        download_path: body.download_path,
        headless: body.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(request, JobPriority::Normal)
        .await;

    info!(
        "Created ETC batch scrape job (immediate): {} for {} accounts",
        job_id, account_count
    );

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "pending".to_string(),
            message: format!(
                "ETC batch scrape job created for {} accounts. Use /v1/job/{{id}} to check status.",
                account_count
            ),
        }),
    )
}

/// ETC batch scrape queue endpoint - runs when idle
async fn handle_etc_scrape_batch_queue(
    State(state): State<Arc<AppState>>,
    Json(body): Json<EtcBatchRequestBody>,
) -> impl IntoResponse {
    let accounts: Vec<EtcAccountInfo> = body
        .accounts
        .into_iter()
        .map(|a| EtcAccountInfo {
            user_id: a.user_id,
            password: a.password,
            name: a.name,
        })
        .collect();

    let account_count = accounts.len();
    let request = EtcBatchRequest {
        accounts,
        download_path: body.download_path,
        headless: body.headless,
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(request, JobPriority::Low)
        .await;

    info!(
        "Created ETC batch scrape job (queued): {} for {} accounts",
        job_id, account_count
    );

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "queued".to_string(),
            message: format!(
                "ETC batch scrape job queued for {} accounts. Will run when system is idle. Use /v1/job/{{id}} to check status.",
                account_count
            ),
        }),
    )
}

/// ETC batch scrape from env endpoint - runs immediately
async fn handle_etc_scrape_batch_env(
    State(state): State<Arc<AppState>>,
    body: Option<Json<EtcBatchEnvRequestBody>>,
) -> impl IntoResponse {
    let body = body.map(|b| b.0).unwrap_or_default();

    let accounts = match get_accounts_from_env() {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse { error: e }),
            )
                .into_response()
        }
    };

    let account_count = accounts.len();
    let request = EtcBatchRequest {
        accounts,
        download_path: body.download_path.unwrap_or_else(default_download_path),
        headless: body.headless.unwrap_or_else(default_headless),
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(request, JobPriority::Normal)
        .await;

    info!(
        "Created ETC batch scrape job from env (immediate): {} for {} accounts",
        job_id, account_count
    );

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "pending".to_string(),
            message: format!(
                "ETC batch scrape job created for {} accounts from env. Use /v1/job/{{id}} to check status.",
                account_count
            ),
        }),
    )
        .into_response()
}

/// ETC batch scrape from env queue endpoint - runs when idle
async fn handle_etc_scrape_batch_env_queue(
    State(state): State<Arc<AppState>>,
    body: Option<Json<EtcBatchEnvRequestBody>>,
) -> impl IntoResponse {
    let body = body.map(|b| b.0).unwrap_or_default();

    let accounts = match get_accounts_from_env() {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse { error: e }),
            )
                .into_response()
        }
    };

    let account_count = accounts.len();
    let request = EtcBatchRequest {
        accounts,
        download_path: body.download_path.unwrap_or_else(default_download_path),
        headless: body.headless.unwrap_or_else(default_headless),
    };

    let job_id = state
        .job_manager
        .create_etc_batch_job(request, JobPriority::Low)
        .await;

    info!(
        "Created ETC batch scrape job from env (queued): {} for {} accounts",
        job_id, account_count
    );

    (
        StatusCode::ACCEPTED,
        Json(VehicleDataResponse {
            job_id,
            status: "queued".to_string(),
            message: format!(
                "ETC batch scrape job queued for {} accounts from env. Will run when system is idle. Use /v1/job/{{id}} to check status.",
                account_count
            ),
        }),
    )
        .into_response()
}

/// Queue status endpoint
async fn handle_queue_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let queue_length = state.job_manager.queue_length().await;
    let running_jobs = state.job_manager.running_job_count().await;
    let is_idle = state.job_manager.is_idle().await;

    Json(QueueStatusResponse {
        queue_length,
        running_jobs,
        is_idle,
    })
}

/// Session check endpoint
async fn handle_session_check(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionQuery>,
) -> impl IntoResponse {
    let session_id = match query.session_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SessionCheckResponse {
                    is_valid: false,
                    message: "Session ID is required".to_string(),
                }),
            )
        }
    };

    let (is_valid, message) = state.renderer.check_session(&session_id).await;

    (StatusCode::OK, Json(SessionCheckResponse { is_valid, message }))
}

/// Session clear endpoint
async fn handle_session_clear(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionQuery>,
) -> impl IntoResponse {
    let session_id = match query.session_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SessionClearResponse {
                    success: false,
                    message: "Session ID is required".to_string(),
                }),
            )
        }
    };

    match state.renderer.clear_session(&session_id).await {
        Ok(_) => (
            StatusCode::OK,
            Json(SessionClearResponse {
                success: true,
                message: "Session cleared successfully".to_string(),
            }),
        ),
        Err(e) => {
            error!("Failed to clear session: {}", e);
            let status_code = StatusCode::from_u16(e.http_status_code())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (
                status_code,
                Json(SessionClearResponse {
                    success: false,
                    message: format!("Failed to clear session: {}", e),
                }),
            )
        }
    }
}

/// Health check endpoint
async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs_f64();

    let etc_accounts = std::env::var("ETC_ACCOUNTS").is_ok();
    let etc_download_path = std::env::var("ETC_DOWNLOAD_PATH").ok();

    Json(HealthResponse {
        status: "healthy".to_string(),
        version: "1.0.0".to_string(),
        uptime,
        env: EnvStatus {
            etc_accounts,
            etc_download_path,
        },
    })
}

/// Metrics endpoint
async fn handle_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs_f64();

    Json(MetricsResponse {
        uptime_seconds: uptime,
        timestamp: chrono::Utc::now().timestamp(),
    })
}

// ========== ETC Session/File Handlers ==========

/// List all session folders
async fn handle_etc_sessions() -> impl IntoResponse {
    let download_path = default_download_path();

    let entries = match std::fs::read_dir(&download_path) {
        Ok(entries) => entries,
        Err(e) => {
            error!("Failed to read download directory {}: {}", download_path, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to read directory: {}", e)})),
            ).into_response();
        }
    };

    let mut sessions: Vec<SessionInfo> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            // Check if it's a session folder (YYYYMMDD_HHMMSS format: 15 chars, _ at position 8)
            if name.len() == 15 && name.chars().nth(8) == Some('_') && entry.path().is_dir() {
                // Count CSV files in the folder
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
                    .unwrap_or(0);
                Some(SessionInfo { name, file_count })
            } else {
                None
            }
        })
        .collect();

    // Sort by name descending (newest first)
    sessions.sort_by(|a, b| b.name.cmp(&a.name));

    (StatusCode::OK, Json(serde_json::json!(SessionsListResponse { sessions }))).into_response()
}

/// List files in a session folder
async fn handle_etc_session_files(
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    // Validate session_id format (YYYYMMDD_HHMMSS)
    if session_id.len() != 15 || session_id.chars().nth(8) != Some('_') {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid session ID format. Expected YYYYMMDD_HHMMSS".to_string(),
            }),
        ).into_response();
    }

    // Prevent path traversal
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid session ID".to_string(),
            }),
        ).into_response();
    }

    let download_path = default_download_path();
    let session_path = std::path::Path::new(&download_path).join(&session_id);

    if !session_path.is_dir() {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session not found: {}", session_id),
            }),
        ).into_response();
    }

    let entries = match std::fs::read_dir(&session_path) {
        Ok(entries) => entries,
        Err(e) => {
            error!("Failed to read session directory {:?}: {}", session_path, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to read session: {}", e),
                }),
            ).into_response();
        }
    };

    let mut files: Vec<FileInfo> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_file() && path.extension().map(|ext| ext.eq_ignore_ascii_case("csv")).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                Some(FileInfo { name, size })
            } else {
                None
            }
        })
        .collect();

    // Sort by name
    files.sort_by(|a, b| a.name.cmp(&b.name));

    (
        StatusCode::OK,
        Json(SessionFilesResponse {
            session_id,
            files,
        }),
    ).into_response()
}

/// Download a file from a session
async fn handle_etc_file_download(
    Path((session_id, filename)): Path<(String, String)>,
) -> impl IntoResponse {
    // Validate session_id format
    if session_id.len() != 15 || session_id.chars().nth(8) != Some('_') {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid session ID format".to_string(),
            }),
        ).into_response();
    }

    // Prevent path traversal
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\')
        || filename.contains("..") || filename.contains('/') || filename.contains('\\')
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid path".to_string(),
            }),
        ).into_response();
    }

    // Only allow CSV files
    if !filename.to_lowercase().ends_with(".csv") {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Only CSV files can be downloaded".to_string(),
            }),
        ).into_response();
    }

    let download_path = default_download_path();
    let file_path = std::path::Path::new(&download_path)
        .join(&session_id)
        .join(&filename);

    if !file_path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("File not found: {}", filename),
            }),
        ).into_response();
    }

    match std::fs::read(&file_path) {
        Ok(contents) => {
            let headers = [
                (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{}\"", filename),
                ),
            ];
            (headers, contents).into_response()
        }
        Err(e) => {
            error!("Failed to read file {:?}: {}", file_path, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to read file: {}", e),
                }),
            ).into_response()
        }
    }
}

/// Start HTTP server
pub async fn start_http_server(state: Arc<AppState>, address: &str) -> anyhow::Result<()> {
    let router = create_router(state);

    info!("HTTP server starting on {}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
