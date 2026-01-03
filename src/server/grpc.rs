use std::sync::Arc;
use std::time::Instant;

use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tracing::info;

use crate::browser::Renderer;
use crate::config::Config;
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
use browser_render::{
    CheckSessionRequest, CheckSessionResponse, ClearSessionRequest, ClearSessionResponse,
    GetVehicleDataRequest, GetVehicleDataResponse, HealthCheckRequest, HealthCheckResponse,
    VehicleData,
};

/// gRPC server implementation
pub struct GrpcServer {
    config: Arc<Config>,
    storage: Arc<Storage>,
    renderer: Arc<Renderer>,
    start_time: Instant,
}

impl GrpcServer {
    pub fn new(
        config: Arc<Config>,
        storage: Arc<Storage>,
        renderer: Arc<Renderer>,
    ) -> Self {
        GrpcServer {
            config,
            storage,
            renderer,
            start_time: Instant::now(),
        }
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

/// Start gRPC server
pub async fn start_grpc_server(
    config: Arc<Config>,
    storage: Arc<Storage>,
    renderer: Arc<Renderer>,
    address: &str,
) -> anyhow::Result<()> {
    let server = GrpcServer::new(config, storage, renderer);

    info!("gRPC server starting on {}", address);

    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(browser_render::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    Server::builder()
        .add_service(reflection_service)
        .add_service(BrowserRenderServiceServer::new(server))
        .serve(address.parse()?)
        .await?;

    Ok(())
}
