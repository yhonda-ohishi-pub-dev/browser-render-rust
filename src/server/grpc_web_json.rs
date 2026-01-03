//! gRPC-Web JSON Mode Layer
//!
//! Provides `application/grpc-web+json` support for gRPC-Web clients.
//! This allows browsers to use JSON payloads instead of Protobuf binary.
//!
//! The layer intercepts requests with `application/grpc-web+json` content type,
//! converts JSON to Protobuf for the gRPC service, and converts the response back to JSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{header, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use futures::future::BoxFuture;
use prost::Message;
use serde::{Deserialize, Serialize};
use tower::{Layer, Service};
use tracing::{debug, error, info, warn};

use super::grpc::browser_render;

/// gRPC-Web JSON Layer
///
/// This layer intercepts `application/grpc-web+json` requests and handles
/// JSON <-> Protobuf conversion.
#[derive(Clone)]
pub struct GrpcWebJsonLayer {
    /// Mapping of gRPC method paths to handler info
    handlers: Arc<HashMap<String, HandlerInfo>>,
}

#[derive(Clone)]
struct HandlerInfo {
    #[allow(dead_code)]
    service: String,
    method: String,
}

impl GrpcWebJsonLayer {
    pub fn new() -> Self {
        let mut handlers = HashMap::new();

        // BrowserRenderService methods
        handlers.insert(
            "/browser_render.v1.BrowserRenderService/GetVehicleData".to_string(),
            HandlerInfo { service: "BrowserRenderService".to_string(), method: "GetVehicleData".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.BrowserRenderService/CheckSession".to_string(),
            HandlerInfo { service: "BrowserRenderService".to_string(), method: "CheckSession".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.BrowserRenderService/ClearSession".to_string(),
            HandlerInfo { service: "BrowserRenderService".to_string(), method: "ClearSession".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.BrowserRenderService/HealthCheck".to_string(),
            HandlerInfo { service: "BrowserRenderService".to_string(), method: "HealthCheck".to_string() },
        );

        // EtcScraperService methods
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrape".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrape".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrapeQueue".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrapeQueue".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrapeBatch".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrapeBatch".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrapeBatchQueue".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrapeBatchQueue".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrapeBatchEnv".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrapeBatchEnv".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/EtcScrapeBatchEnvQueue".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "EtcScrapeBatchEnvQueue".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/GetJob".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "GetJob".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/ListJobs".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "ListJobs".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/GetQueueStatus".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "GetQueueStatus".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/ListSessions".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "ListSessions".to_string() },
        );
        handlers.insert(
            "/browser_render.v1.EtcScraperService/ListSessionFiles".to_string(),
            HandlerInfo { service: "EtcScraperService".to_string(), method: "ListSessionFiles".to_string() },
        );

        GrpcWebJsonLayer {
            handlers: Arc::new(handlers),
        }
    }
}

impl Default for GrpcWebJsonLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for GrpcWebJsonLayer {
    type Service = GrpcWebJsonService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GrpcWebJsonService {
            inner,
            handlers: Arc::clone(&self.handlers),
        }
    }
}

/// gRPC-Web JSON Service wrapper
#[derive(Clone)]
pub struct GrpcWebJsonService<S> {
    inner: S,
    handlers: Arc<HashMap<String, HandlerInfo>>,
}

/// UnsyncBoxBody type alias for response body (matches tonic's body type)
type UnsyncBoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, tonic::Status>;

fn boxed_body<B>(body: B) -> UnsyncBoxBody
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    use http_body_util::BodyExt;
    body.map_err(|e| tonic::Status::internal(format!("{:?}", e.into())))
        .boxed_unsync()
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for GrpcWebJsonService<S>
where
    S: Service<Request<UnsyncBoxBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: http_body::Body<Data = Bytes> + Send + 'static,
    ReqBody::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    ResBody: http_body::Body<Data = Bytes> + Send + 'static,
    ResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = Response<UnsyncBoxBody>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Check if this is a gRPC-Web JSON request
        let is_grpc_web_json = content_type.starts_with("application/grpc-web+json")
            || content_type.starts_with("application/grpc-web-text+json");

        let handlers = Arc::clone(&self.handlers);
        let mut inner = self.inner.clone();

        if !is_grpc_web_json {
            // Pass through to inner service (convert body type)
            let (parts, body) = req.into_parts();
            let boxed = boxed_body(body);
            let new_req = Request::from_parts(parts, boxed);

            return Box::pin(async move {
                let resp = inner.call(new_req).await?;
                let (parts, body) = resp.into_parts();
                Ok(Response::from_parts(parts, boxed_body(body)))
            });
        }

        let path = req.uri().path().to_string();

        Box::pin(async move {
            info!("gRPC-Web+JSON request: {}", path);

            // Check if we have a handler for this path
            let handler_info = match handlers.get(&path) {
                Some(info) => info,
                None => {
                    warn!("Unknown gRPC-Web+JSON path: {}", path);
                    return Ok(create_error_response(&format!("Unknown method: {}", path)));
                }
            };

            // Read request body
            let (parts, body) = req.into_parts();
            let body_bytes = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    error!("Failed to read request body: {:?}", e.into());
                    return Ok(create_error_response("Failed to read request body"));
                }
            };

            // For gRPC-Web, the body has a 5-byte header: 1 byte flag + 4 bytes length
            // But for JSON mode, we need to extract the JSON payload
            let json_payload = if body_bytes.len() > 5 {
                // Skip the 5-byte gRPC frame header if present
                let flag = body_bytes[0];
                if flag == 0 || flag == 1 {
                    // This looks like a gRPC frame
                    let len = u32::from_be_bytes([body_bytes[1], body_bytes[2], body_bytes[3], body_bytes[4]]) as usize;
                    if body_bytes.len() >= 5 + len {
                        body_bytes.slice(5..5 + len)
                    } else {
                        body_bytes.clone()
                    }
                } else {
                    // No frame header, use raw body
                    body_bytes.clone()
                }
            } else {
                body_bytes.clone()
            };

            debug!("JSON payload: {}", String::from_utf8_lossy(&json_payload));

            // Convert JSON to Protobuf based on method
            let proto_bytes = match convert_json_to_proto(&handler_info.method, &json_payload) {
                Ok(bytes) => bytes,
                Err(e) => {
                    error!("Failed to convert JSON to Protobuf: {}", e);
                    return Ok(create_error_response(&format!("Invalid JSON: {}", e)));
                }
            };

            // Create gRPC-Web binary frame (1 byte flag + 4 bytes length + data)
            let mut grpc_frame = Vec::with_capacity(5 + proto_bytes.len());
            grpc_frame.push(0); // Flag: 0 = data frame
            grpc_frame.extend_from_slice(&(proto_bytes.len() as u32).to_be_bytes());
            grpc_frame.extend_from_slice(&proto_bytes);

            // Build new request with protobuf content
            let mut new_req = Request::builder()
                .method(Method::POST)
                .uri(parts.uri)
                .header(header::CONTENT_TYPE, "application/grpc-web+proto")
                .header("x-grpc-web", "1")
                .body(boxed_body(Full::new(Bytes::from(grpc_frame))))
                .unwrap();

            // Copy other headers
            for (key, value) in parts.headers.iter() {
                if key != header::CONTENT_TYPE && key != header::CONTENT_LENGTH {
                    new_req.headers_mut().insert(key.clone(), value.clone());
                }
            }

            // Call inner service
            let response = inner.call(new_req).await?;

            // Convert response back to JSON
            let (resp_parts, resp_body) = response.into_parts();
            let resp_bytes = match resp_body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    error!("Failed to read response body: {:?}", e.into());
                    return Ok(create_error_response("Failed to read response"));
                }
            };

            // Parse gRPC-Web response frame
            if resp_bytes.len() < 5 {
                return Ok(create_grpc_json_response(&handler_info.method, &[]));
            }

            let flag = resp_bytes[0];
            let len = u32::from_be_bytes([resp_bytes[1], resp_bytes[2], resp_bytes[3], resp_bytes[4]]) as usize;

            if flag == 0 && resp_bytes.len() >= 5 + len {
                // Data frame - convert to JSON
                let proto_data = &resp_bytes[5..5 + len];
                match convert_proto_to_json(&handler_info.method, proto_data) {
                    Ok(json) => {
                        debug!("Response JSON: {}", json);

                        // Create gRPC-Web JSON frame
                        let mut json_frame = Vec::with_capacity(5 + json.len());
                        json_frame.push(0); // Flag: 0 = data frame
                        json_frame.extend_from_slice(&(json.len() as u32).to_be_bytes());
                        json_frame.extend_from_slice(json.as_bytes());

                        // Add trailers frame if present
                        let trailer_start = 5 + len;
                        if resp_bytes.len() > trailer_start {
                            json_frame.extend_from_slice(&resp_bytes[trailer_start..]);
                        } else {
                            // Add default OK trailers
                            let trailers = "grpc-status:0\r\n";
                            json_frame.push(0x80); // Flag: trailer frame
                            json_frame.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
                            json_frame.extend_from_slice(trailers.as_bytes());
                        }

                        let mut response = Response::new(boxed_body(Full::new(Bytes::from(json_frame))));
                        *response.status_mut() = resp_parts.status;
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/grpc-web+json"),
                        );
                        // Copy CORS headers
                        for (key, value) in resp_parts.headers.iter() {
                            if key.as_str().starts_with("access-control") {
                                response.headers_mut().insert(key.clone(), value.clone());
                            }
                        }
                        Ok(response)
                    }
                    Err(e) => {
                        error!("Failed to convert Protobuf to JSON: {}", e);
                        Ok(create_error_response(&format!("Failed to convert response: {}", e)))
                    }
                }
            } else {
                // Trailer frame or error - pass through with modified content type
                let mut response = Response::new(boxed_body(Full::new(resp_bytes)));
                *response.status_mut() = resp_parts.status;
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/grpc-web+json"),
                );
                Ok(response)
            }
        })
    }
}

fn create_error_response(message: &str) -> Response<UnsyncBoxBody> {
    let error_json = serde_json::json!({ "error": message });
    let json_str = error_json.to_string();

    // Create gRPC-Web frame with error
    let mut frame = Vec::with_capacity(5 + json_str.len() + 50);
    frame.push(0); // Data frame
    frame.extend_from_slice(&(json_str.len() as u32).to_be_bytes());
    frame.extend_from_slice(json_str.as_bytes());

    // Add error trailer
    let trailers = format!("grpc-status:2\r\ngrpc-message:{}\r\n", message);
    frame.push(0x80); // Trailer frame
    frame.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
    frame.extend_from_slice(trailers.as_bytes());

    let mut response = Response::new(boxed_body(Full::new(Bytes::from(frame))));
    *response.status_mut() = StatusCode::OK; // gRPC always returns 200
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc-web+json"),
    );
    response
}

fn create_grpc_json_response(_method: &str, _proto_data: &[u8]) -> Response<UnsyncBoxBody> {
    let json_str = "{}";

    let mut frame = Vec::with_capacity(5 + json_str.len() + 20);
    frame.push(0);
    frame.extend_from_slice(&(json_str.len() as u32).to_be_bytes());
    frame.extend_from_slice(json_str.as_bytes());

    let trailers = "grpc-status:0\r\n";
    frame.push(0x80);
    frame.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
    frame.extend_from_slice(trailers.as_bytes());

    let mut response = Response::new(boxed_body(Full::new(Bytes::from(frame))));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc-web+json"),
    );
    response
}

// ========== JSON <-> Protobuf Conversion ==========

/// Convert JSON request to Protobuf bytes
fn convert_json_to_proto(method: &str, json_bytes: &[u8]) -> Result<Vec<u8>, String> {
    // Handle empty body
    if json_bytes.is_empty() || json_bytes == b"{}" || json_bytes == b"null" {
        return match method {
            "HealthCheck" => Ok(browser_render::HealthCheckRequest {}.encode_to_vec()),
            "ListJobs" => Ok(browser_render::ListJobsRequest::default().encode_to_vec()),
            "GetQueueStatus" => Ok(browser_render::GetQueueStatusRequest {}.encode_to_vec()),
            "ListSessions" => Ok(browser_render::ListSessionsRequest {}.encode_to_vec()),
            "EtcScrapeBatchEnv" | "EtcScrapeBatchEnvQueue" => {
                Ok(browser_render::EtcBatchEnvRequest::default().encode_to_vec())
            }
            _ => Err(format!("Empty body not allowed for method: {}", method)),
        };
    }

    match method {
        // BrowserRenderService
        "GetVehicleData" => {
            let req: GetVehicleDataJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::GetVehicleDataRequest {
                branch_id: req.branch_id.unwrap_or_default(),
                filter_id: req.filter_id.unwrap_or_default(),
                force_login: req.force_login.unwrap_or(false),
            }.encode_to_vec())
        }
        "CheckSession" => {
            let req: CheckSessionJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::CheckSessionRequest {
                session_id: req.session_id.unwrap_or_default(),
            }.encode_to_vec())
        }
        "ClearSession" => {
            let req: ClearSessionJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::ClearSessionRequest {
                session_id: req.session_id.unwrap_or_default(),
            }.encode_to_vec())
        }
        "HealthCheck" => {
            Ok(browser_render::HealthCheckRequest {}.encode_to_vec())
        }

        // EtcScraperService
        "EtcScrape" | "EtcScrapeQueue" => {
            let req: EtcScrapeJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::EtcScrapeRequest {
                user_id: req.user_id.unwrap_or_default(),
                password: req.password.unwrap_or_default(),
                download_path: req.download_path.unwrap_or_default(),
                headless: req.headless.unwrap_or(true),
            }.encode_to_vec())
        }
        "EtcScrapeBatch" | "EtcScrapeBatchQueue" => {
            let req: EtcBatchJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::EtcBatchRequest {
                accounts: req.accounts.unwrap_or_default().into_iter().map(|a| {
                    browser_render::EtcAccountInfo {
                        user_id: a.user_id.unwrap_or_default(),
                        password: a.password.unwrap_or_default(),
                        name: a.name.unwrap_or_default(),
                    }
                }).collect(),
                download_path: req.download_path.unwrap_or_default(),
                headless: req.headless.unwrap_or(true),
            }.encode_to_vec())
        }
        "EtcScrapeBatchEnv" | "EtcScrapeBatchEnvQueue" => {
            let req: EtcBatchEnvJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::EtcBatchEnvRequest {
                download_path: req.download_path.unwrap_or_default(),
                headless: req.headless.unwrap_or(true),
                headless_set: req.headless.is_some(),
            }.encode_to_vec())
        }
        "GetJob" => {
            let req: GetJobJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::GetJobRequest {
                job_id: req.job_id.unwrap_or_default(),
            }.encode_to_vec())
        }
        "ListJobs" => {
            let req: ListJobsJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::ListJobsRequest {
                job_type: req.job_type.unwrap_or_default(),
                status: req.status.unwrap_or_default(),
            }.encode_to_vec())
        }
        "GetQueueStatus" => {
            Ok(browser_render::GetQueueStatusRequest {}.encode_to_vec())
        }
        "ListSessions" => {
            Ok(browser_render::ListSessionsRequest {}.encode_to_vec())
        }
        "ListSessionFiles" => {
            let req: ListSessionFilesJson = serde_json::from_slice(json_bytes)
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok(browser_render::ListSessionFilesRequest {
                session_id: req.session_id.unwrap_or_default(),
            }.encode_to_vec())
        }
        _ => Err(format!("Unknown method: {}", method)),
    }
}

/// Convert Protobuf response to JSON string
fn convert_proto_to_json(method: &str, proto_bytes: &[u8]) -> Result<String, String> {
    match method {
        // BrowserRenderService
        "GetVehicleData" => {
            let resp = browser_render::GetVehicleDataResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = GetVehicleDataResponseJson {
                status: resp.status,
                status_code: resp.status_code,
                data: resp.data.into_iter().map(|v| VehicleDataJson {
                    vehicle_cd: v.vehicle_cd,
                    vehicle_name: v.vehicle_name,
                    status: v.status,
                    metadata: v.metadata,
                }).collect(),
                session_id: resp.session_id,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "CheckSession" => {
            let resp = browser_render::CheckSessionResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = CheckSessionResponseJson {
                is_valid: resp.is_valid,
                message: resp.message,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "ClearSession" => {
            let resp = browser_render::ClearSessionResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = ClearSessionResponseJson {
                success: resp.success,
                message: resp.message,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "HealthCheck" => {
            let resp = browser_render::HealthCheckResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = HealthCheckResponseJson {
                status: resp.status,
                version: resp.version,
                uptime: resp.uptime,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }

        // EtcScraperService
        "EtcScrape" | "EtcScrapeQueue" | "EtcScrapeBatch" | "EtcScrapeBatchQueue"
        | "EtcScrapeBatchEnv" | "EtcScrapeBatchEnvQueue" => {
            let resp = browser_render::EtcScrapeResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = EtcScrapeResponseJson {
                job_id: resp.job_id,
                status: resp.status,
                message: resp.message,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "GetJob" => {
            let resp = browser_render::GetJobResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = GetJobResponseJson {
                found: resp.found,
                job: resp.job.map(job_proto_to_json),
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "ListJobs" => {
            let resp = browser_render::ListJobsResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = ListJobsResponseJson {
                jobs: resp.jobs.into_iter().map(job_proto_to_json).collect(),
                count: resp.count,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "GetQueueStatus" => {
            let resp = browser_render::GetQueueStatusResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = GetQueueStatusResponseJson {
                queue_length: resp.queue_length,
                running_jobs: resp.running_jobs,
                is_idle: resp.is_idle,
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "ListSessions" => {
            let resp = browser_render::ListSessionsResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = ListSessionsResponseJson {
                sessions: resp.sessions.into_iter().map(|s| SessionInfoJson {
                    name: s.name,
                    file_count: s.file_count,
                }).collect(),
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        "ListSessionFiles" => {
            let resp = browser_render::ListSessionFilesResponse::decode(proto_bytes)
                .map_err(|e| format!("Protobuf decode error: {}", e))?;
            let json = ListSessionFilesResponseJson {
                session_id: resp.session_id,
                files: resp.files.into_iter().map(|f| FileInfoJson {
                    name: f.name,
                    size: f.size,
                }).collect(),
            };
            serde_json::to_string(&json).map_err(|e| format!("JSON serialize error: {}", e))
        }
        _ => Err(format!("Unknown method: {}", method)),
    }
}

fn job_proto_to_json(job: browser_render::Job) -> JobJson {
    JobJson {
        id: job.id,
        job_type: job.job_type,
        priority: job.priority,
        status: job.status,
        created_at: job.created_at,
        started_at: if job.started_at.is_empty() { None } else { Some(job.started_at) },
        completed_at: if job.completed_at.is_empty() { None } else { Some(job.completed_at) },
        error: if job.error.is_empty() { None } else { Some(job.error) },
        etc_result: job.etc_result.map(|r| EtcScrapeResultJson {
            csv_path: r.csv_path,
            csv_size: r.csv_size,
        }),
        batch_result: job.batch_result.map(|r| EtcBatchResultJson {
            session_folder: r.session_folder,
            accounts: r.accounts.into_iter().map(|a| AccountResultJson {
                user_id: a.user_id,
                name: if a.name.is_empty() { None } else { Some(a.name) },
                status: a.status,
                csv_path: if a.csv_path.is_empty() { None } else { Some(a.csv_path) },
                csv_size: if a.csv_size == 0 { None } else { Some(a.csv_size) },
                error: if a.error.is_empty() { None } else { Some(a.error) },
            }).collect(),
            total_count: r.total_count,
            success_count: r.success_count,
            fail_count: r.fail_count,
        }),
        current_account_index: if job.current_account_index == 0 { None } else { Some(job.current_account_index) },
    }
}

// ========== JSON Types ==========

// Request types
#[derive(Deserialize)]
struct GetVehicleDataJson {
    branch_id: Option<String>,
    filter_id: Option<String>,
    force_login: Option<bool>,
}

#[derive(Deserialize)]
struct CheckSessionJson {
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct ClearSessionJson {
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct EtcScrapeJson {
    user_id: Option<String>,
    password: Option<String>,
    download_path: Option<String>,
    headless: Option<bool>,
}

#[derive(Deserialize)]
struct EtcAccountJson {
    user_id: Option<String>,
    password: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct EtcBatchJson {
    accounts: Option<Vec<EtcAccountJson>>,
    download_path: Option<String>,
    headless: Option<bool>,
}

#[derive(Deserialize)]
struct EtcBatchEnvJson {
    download_path: Option<String>,
    headless: Option<bool>,
}

#[derive(Deserialize)]
struct GetJobJson {
    job_id: Option<String>,
}

#[derive(Deserialize)]
struct ListJobsJson {
    job_type: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct ListSessionFilesJson {
    session_id: Option<String>,
}

// Response types
#[derive(Serialize)]
struct GetVehicleDataResponseJson {
    status: String,
    status_code: i32,
    data: Vec<VehicleDataJson>,
    session_id: String,
}

#[derive(Serialize)]
struct VehicleDataJson {
    vehicle_cd: String,
    vehicle_name: String,
    status: String,
    metadata: HashMap<String, String>,
}

#[derive(Serialize)]
struct CheckSessionResponseJson {
    is_valid: bool,
    message: String,
}

#[derive(Serialize)]
struct ClearSessionResponseJson {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct HealthCheckResponseJson {
    status: String,
    version: String,
    uptime: i64,
}

#[derive(Serialize)]
struct EtcScrapeResponseJson {
    job_id: String,
    status: String,
    message: String,
}

#[derive(Serialize)]
struct GetJobResponseJson {
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    job: Option<JobJson>,
}

#[derive(Serialize)]
struct ListJobsResponseJson {
    jobs: Vec<JobJson>,
    count: i32,
}

#[derive(Serialize)]
struct GetQueueStatusResponseJson {
    queue_length: i32,
    running_jobs: i32,
    is_idle: bool,
}

#[derive(Serialize)]
struct ListSessionsResponseJson {
    sessions: Vec<SessionInfoJson>,
}

#[derive(Serialize)]
struct SessionInfoJson {
    name: String,
    file_count: i32,
}

#[derive(Serialize)]
struct ListSessionFilesResponseJson {
    session_id: String,
    files: Vec<FileInfoJson>,
}

#[derive(Serialize)]
struct FileInfoJson {
    name: String,
    size: i64,
}

#[derive(Serialize)]
struct JobJson {
    id: String,
    job_type: String,
    priority: String,
    status: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etc_result: Option<EtcScrapeResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    batch_result: Option<EtcBatchResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_account_index: Option<i32>,
}

#[derive(Serialize)]
struct EtcScrapeResultJson {
    csv_path: String,
    csv_size: i32,
}

#[derive(Serialize)]
struct EtcBatchResultJson {
    session_folder: String,
    accounts: Vec<AccountResultJson>,
    total_count: i32,
    success_count: i32,
    fail_count: i32,
}

#[derive(Serialize)]
struct AccountResultJson {
    user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    csv_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    csv_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}
