use std::sync::Arc;
use std::time::Instant;

use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tonic_web::GrpcWebLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

use super::grpc_web_json::GrpcWebJsonLayer;

use crate::browser::Renderer;
use crate::config::Config;
use crate::jobs::{JobManager, JobPriority};
use crate::storage::Storage;

// Protocol buffer definitions (inline for now, can be generated from .proto file)
pub mod browser_render {
    tonic::include_proto!("browser_render.v1");

    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("browser_render_descriptor");
}

use browser_render::browser_render_service_server::{
    BrowserRenderService, BrowserRenderServiceServer,
};
use browser_render::etc_scraper_service_server::{EtcScraperService, EtcScraperServiceServer};
use browser_render::{
    CheckSessionRequest, CheckSessionResponse, ClearSessionRequest, ClearSessionResponse,
    GetVehicleDataRequest, GetVehicleDataResponse, HealthCheckRequest, HealthCheckResponse,
    VehicleData,
    // ETC Scraper types
    EtcScrapeRequest as ProtoEtcScrapeRequest, EtcScrapeResponse,
    EtcBatchRequest as ProtoEtcBatchRequest, EtcBatchEnvRequest,
    GetJobRequest, GetJobResponse, ListJobsRequest, ListJobsResponse,
    GetQueueStatusRequest, GetQueueStatusResponse,
    ListSessionsRequest, ListSessionsResponse, ListSessionFilesRequest, ListSessionFilesResponse,
    Job as ProtoJob, EtcScrapeResult as ProtoEtcScrapeResult, EtcBatchResult as ProtoEtcBatchResult,
    AccountResult as ProtoAccountResult, SessionInfo, FileInfo,
};

/// gRPC server implementation
pub struct GrpcServer {
    config: Arc<Config>,
    storage: Arc<Storage>,
    renderer: Arc<Renderer>,
    job_manager: Arc<JobManager>,
    start_time: Instant,
}

impl GrpcServer {
    pub fn new(
        config: Arc<Config>,
        storage: Arc<Storage>,
        renderer: Arc<Renderer>,
        job_manager: Arc<JobManager>,
    ) -> Self {
        GrpcServer {
            config,
            storage,
            renderer,
            job_manager,
            start_time: Instant::now(),
        }
    }
}

// Helper functions
fn default_download_path() -> String {
    std::env::var("ETC_DOWNLOAD_PATH").unwrap_or_else(|_| "./downloads".to_string())
}

fn get_accounts_from_env() -> Result<Vec<crate::jobs::EtcAccountInfo>, String> {
    let accounts_json = std::env::var("ETC_ACCOUNTS")
        .map_err(|_| "ETC_ACCOUNTS environment variable not set".to_string())?;

    #[derive(serde::Deserialize)]
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

/// Convert internal Job to Proto Job
fn job_to_proto(job: &crate::jobs::Job) -> ProtoJob {
    ProtoJob {
        id: job.id.clone(),
        job_type: format!("{:?}", job.job_type),
        priority: format!("{:?}", job.priority),
        status: format!("{:?}", job.status),
        created_at: job.created_at.to_rfc3339(),
        started_at: job.started_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        completed_at: job.completed_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
        error: job.error.clone().unwrap_or_default(),
        etc_result: job.etc_result.as_ref().map(|r| ProtoEtcScrapeResult {
            csv_path: r.csv_path.clone(),
            csv_size: r.csv_size as i32,
        }),
        batch_result: job.batch_result.as_ref().map(|r| ProtoEtcBatchResult {
            session_folder: r.session_folder.clone(),
            accounts: r.accounts.iter().map(|a| ProtoAccountResult {
                user_id: a.user_id.clone(),
                name: a.name.clone(),
                status: format!("{:?}", a.status),
                csv_path: a.csv_path.clone().unwrap_or_default(),
                csv_size: a.csv_size.unwrap_or(0) as i32,
                error: a.error.clone().unwrap_or_default(),
            }).collect(),
            total_count: r.total_count as i32,
            success_count: r.success_count as i32,
            fail_count: r.fail_count as i32,
        }),
        current_account_index: job.current_account_index.unwrap_or(0) as i32,
    }
}

#[tonic::async_trait]
impl BrowserRenderService for GrpcServer {
    async fn get_vehicle_data(
        &self,
        request: Request<GetVehicleDataRequest>,
    ) -> Result<Response<GetVehicleDataResponse>, Status> {
        let req = request.into_inner();
        info!(
            "GetVehicleData called with branchId={}, filterId={}",
            req.branch_id, req.filter_id
        );

        match self
            .renderer
            .get_vehicle_data("", &req.branch_id, &req.filter_id, req.force_login)
            .await
        {
            Ok((vehicles, session_id, _)) => {
                let pb_vehicles: Vec<VehicleData> = vehicles
                    .into_iter()
                    .map(|v| VehicleData {
                        vehicle_cd: v.vehicle_cd,
                        vehicle_name: v.vehicle_name,
                        status: v.status,
                        metadata: v.metadata,
                    })
                    .collect();

                Ok(Response::new(GetVehicleDataResponse {
                    status: "success".to_string(),
                    status_code: 200,
                    data: pb_vehicles,
                    session_id,
                }))
            }
            Err(e) => Ok(Response::new(GetVehicleDataResponse {
                status: e.to_string(),
                status_code: 500,
                data: vec![],
                session_id: String::new(),
            })),
        }
    }

    async fn check_session(
        &self,
        request: Request<CheckSessionRequest>,
    ) -> Result<Response<CheckSessionResponse>, Status> {
        let req = request.into_inner();
        info!("CheckSession called with sessionId={}", req.session_id);

        if req.session_id.is_empty() {
            return Ok(Response::new(CheckSessionResponse {
                is_valid: false,
                message: "Session ID is required".to_string(),
            }));
        }

        let (is_valid, message) = self.renderer.check_session(&req.session_id).await;

        Ok(Response::new(CheckSessionResponse { is_valid, message }))
    }

    async fn clear_session(
        &self,
        request: Request<ClearSessionRequest>,
    ) -> Result<Response<ClearSessionResponse>, Status> {
        let req = request.into_inner();
        info!("ClearSession called with sessionId={}", req.session_id);

        if req.session_id.is_empty() {
            return Ok(Response::new(ClearSessionResponse {
                success: false,
                message: "Session ID is required".to_string(),
            }));
        }

        match self.renderer.clear_session(&req.session_id).await {
            Ok(_) => Ok(Response::new(ClearSessionResponse {
                success: true,
                message: "Session cleared successfully".to_string(),
            })),
            Err(e) => Ok(Response::new(ClearSessionResponse {
                success: false,
                message: format!("Failed to clear session: {}", e),
            })),
        }
    }

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let uptime = self.start_time.elapsed().as_secs() as i64;

        Ok(Response::new(HealthCheckResponse {
            status: "healthy".to_string(),
            version: "1.0.0".to_string(),
            uptime,
        }))
    }
}

#[tonic::async_trait]
impl EtcScraperService for GrpcServer {
    async fn etc_scrape(
        &self,
        request: Request<ProtoEtcScrapeRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        info!("EtcScrape called for user_id={}", req.user_id);

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

        let job_id = self
            .job_manager
            .create_etc_job(etc_request, JobPriority::Normal)
            .await;

        info!("Created ETC scrape job (immediate): {} for user {}", job_id, req.user_id);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "pending".to_string(),
            message: "ETC scrape job created. Use GetJob to check status.".to_string(),
        }))
    }

    async fn etc_scrape_queue(
        &self,
        request: Request<ProtoEtcScrapeRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        info!("EtcScrapeQueue called for user_id={}", req.user_id);

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

        let job_id = self
            .job_manager
            .create_etc_job(etc_request, JobPriority::Low)
            .await;

        info!("Created ETC scrape job (queued): {} for user {}", job_id, req.user_id);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "queued".to_string(),
            message: "ETC scrape job queued. Will run when system is idle.".to_string(),
        }))
    }

    async fn etc_scrape_batch(
        &self,
        request: Request<ProtoEtcBatchRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        let account_count = req.accounts.len();
        info!("EtcScrapeBatch called for {} accounts", account_count);

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

        let job_id = self
            .job_manager
            .create_etc_batch_job(batch_request, JobPriority::Normal)
            .await;

        info!("Created ETC batch scrape job (immediate): {} for {} accounts", job_id, account_count);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "pending".to_string(),
            message: format!("ETC batch scrape job created for {} accounts.", account_count),
        }))
    }

    async fn etc_scrape_batch_queue(
        &self,
        request: Request<ProtoEtcBatchRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        let account_count = req.accounts.len();
        info!("EtcScrapeBatchQueue called for {} accounts", account_count);

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

        let job_id = self
            .job_manager
            .create_etc_batch_job(batch_request, JobPriority::Low)
            .await;

        info!("Created ETC batch scrape job (queued): {} for {} accounts", job_id, account_count);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "queued".to_string(),
            message: format!("ETC batch scrape job queued for {} accounts.", account_count),
        }))
    }

    async fn etc_scrape_batch_env(
        &self,
        request: Request<EtcBatchEnvRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        info!("EtcScrapeBatchEnv called");

        let accounts = match get_accounts_from_env() {
            Ok(a) => a,
            Err(e) => {
                return Err(Status::invalid_argument(e));
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

        let job_id = self
            .job_manager
            .create_etc_batch_job(batch_request, JobPriority::Normal)
            .await;

        info!("Created ETC batch scrape job from env (immediate): {} for {} accounts", job_id, account_count);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "pending".to_string(),
            message: format!("ETC batch scrape job created for {} accounts from env.", account_count),
        }))
    }

    async fn etc_scrape_batch_env_queue(
        &self,
        request: Request<EtcBatchEnvRequest>,
    ) -> Result<Response<EtcScrapeResponse>, Status> {
        let req = request.into_inner();
        info!("EtcScrapeBatchEnvQueue called");

        let accounts = match get_accounts_from_env() {
            Ok(a) => a,
            Err(e) => {
                return Err(Status::invalid_argument(e));
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

        let job_id = self
            .job_manager
            .create_etc_batch_job(batch_request, JobPriority::Low)
            .await;

        info!("Created ETC batch scrape job from env (queued): {} for {} accounts", job_id, account_count);

        Ok(Response::new(EtcScrapeResponse {
            job_id,
            status: "queued".to_string(),
            message: format!("ETC batch scrape job queued for {} accounts from env.", account_count),
        }))
    }

    async fn get_job(
        &self,
        request: Request<GetJobRequest>,
    ) -> Result<Response<GetJobResponse>, Status> {
        let req = request.into_inner();
        info!("GetJob called for job_id={}", req.job_id);

        match self.job_manager.get_job(&req.job_id).await {
            Some(job) => Ok(Response::new(GetJobResponse {
                found: true,
                job: Some(job_to_proto(&job)),
            })),
            None => Ok(Response::new(GetJobResponse {
                found: false,
                job: None,
            })),
        }
    }

    async fn list_jobs(
        &self,
        _request: Request<ListJobsRequest>,
    ) -> Result<Response<ListJobsResponse>, Status> {
        info!("ListJobs called");

        let jobs = self.job_manager.get_all_jobs().await;
        let count = jobs.len() as i32;
        let proto_jobs: Vec<ProtoJob> = jobs.iter().map(job_to_proto).collect();

        Ok(Response::new(ListJobsResponse {
            jobs: proto_jobs,
            count,
        }))
    }

    async fn get_queue_status(
        &self,
        _request: Request<GetQueueStatusRequest>,
    ) -> Result<Response<GetQueueStatusResponse>, Status> {
        info!("GetQueueStatus called");

        let queue_length = self.job_manager.queue_length().await as i32;
        let running_jobs = self.job_manager.running_job_count().await as i32;
        let is_idle = self.job_manager.is_idle().await;

        Ok(Response::new(GetQueueStatusResponse {
            queue_length,
            running_jobs,
            is_idle,
        }))
    }

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        info!("ListSessions called");

        let download_path = default_download_path();

        let entries = match std::fs::read_dir(&download_path) {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to read download directory {}: {}", download_path, e);
                return Err(Status::internal(format!("Failed to read directory: {}", e)));
            }
        };

        let mut sessions: Vec<SessionInfo> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                // Check if it's a session folder (YYYYMMDD_HHMMSS format: 15 chars, _ at position 8)
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

        // Sort by name descending (newest first)
        sessions.sort_by(|a, b| b.name.cmp(&a.name));

        Ok(Response::new(ListSessionsResponse { sessions }))
    }

    async fn list_session_files(
        &self,
        request: Request<ListSessionFilesRequest>,
    ) -> Result<Response<ListSessionFilesResponse>, Status> {
        let req = request.into_inner();
        info!("ListSessionFiles called for session_id={}", req.session_id);

        // Validate session_id format (YYYYMMDD_HHMMSS)
        if req.session_id.len() != 15 || req.session_id.chars().nth(8) != Some('_') {
            return Err(Status::invalid_argument(
                "Invalid session ID format. Expected YYYYMMDD_HHMMSS",
            ));
        }

        // Prevent path traversal
        if req.session_id.contains("..") || req.session_id.contains('/') || req.session_id.contains('\\') {
            return Err(Status::invalid_argument("Invalid session ID"));
        }

        let download_path = default_download_path();
        let session_path = std::path::Path::new(&download_path).join(&req.session_id);

        if !session_path.is_dir() {
            return Err(Status::not_found(format!(
                "Session not found: {}",
                req.session_id
            )));
        }

        let entries = match std::fs::read_dir(&session_path) {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to read session directory {:?}: {}", session_path, e);
                return Err(Status::internal(format!("Failed to read session: {}", e)));
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

        // Sort by name
        files.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Response::new(ListSessionFilesResponse {
            session_id: req.session_id,
            files,
        }))
    }
}

/// Start gRPC server
pub async fn start_grpc_server(
    config: Arc<Config>,
    storage: Arc<Storage>,
    renderer: Arc<Renderer>,
    job_manager: Arc<JobManager>,
    address: &str,
) -> anyhow::Result<()> {
    let server = GrpcServer::new(
        Arc::clone(&config),
        Arc::clone(&storage),
        Arc::clone(&renderer),
        Arc::clone(&job_manager),
    );
    let server2 = GrpcServer::new(config, storage, renderer, job_manager);

    info!("gRPC server starting on {} (gRPC-web enabled, gRPC-web+json enabled)", address);

    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(browser_render::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    // CORS configuration for gRPC-web (allows browser and Cloudflare Tunnel access)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any)
        .expose_headers(Any);

    Server::builder()
        .accept_http1(true) // Required for gRPC-web
        .layer(cors)
        .layer(GrpcWebJsonLayer::new()) // JSON mode layer (must be before GrpcWebLayer)
        .layer(GrpcWebLayer::new())     // Standard gRPC-web (protobuf)
        .add_service(reflection_service)
        .add_service(BrowserRenderServiceServer::new(server))
        .add_service(EtcScraperServiceServer::new(server2))
        .serve(address.parse()?)
        .await?;

    Ok(())
}
