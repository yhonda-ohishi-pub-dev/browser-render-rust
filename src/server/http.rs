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
use crate::jobs::{EtcScrapeRequest, JobManager, JobPriority};
use crate::storage::Storage;

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

    Router::new()
        // API endpoints
        .route("/v1/vehicle/data", get(handle_vehicle_data))
        .route("/v1/etc/scrape", post(handle_etc_scrape))
        .route("/v1/etc/scrape/queue", post(handle_etc_scrape_queue))
        .route("/v1/job/:id", get(handle_job_status))
        .route("/v1/jobs", get(handle_jobs_list))
        .route("/v1/jobs/queue", get(handle_queue_status))
        .route("/v1/session/check", get(handle_session_check))
        .route("/v1/session/clear", delete(handle_session_clear))
        // Health and metrics
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
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
    "./downloads".to_string()
}

fn default_headless() -> bool {
    true
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

    Json(HealthResponse {
        status: "healthy".to_string(),
        version: "1.0.0".to_string(),
        uptime,
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

/// Start HTTP server
pub async fn start_http_server(state: Arc<AppState>, address: &str) -> anyhow::Result<()> {
    let router = create_router(state);

    info!("HTTP server starting on {}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
