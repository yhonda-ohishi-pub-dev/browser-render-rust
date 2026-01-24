use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Utc, offset::FixedOffset};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::Page;
use futures::StreamExt;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::storage::{Cookie, Session, Storage};

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Browser error during {context}: {message}")]
    Browser { context: &'static str, message: String },
    #[error("Login failed: {0}")]
    LoginFailed(String),
    #[error("Navigation failed to {url}: {message}")]
    NavigationFailed { url: String, message: String },
    #[error("Data extraction failed: {0}")]
    ExtractionFailed(String),
    #[error("API error ({status_code}): {message}")]
    ApiError { status_code: u16, message: String },
    #[error("Session error: {0}")]
    SessionError(String),
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
    #[error("HTTP request error: {0}")]
    HttpRequest(String),
    #[error("JSON serialization error: {0}")]
    JsonError(String),
    #[error("gRPC error: {0}")]
    GrpcError(String),
}

impl RendererError {
    /// Create a browser error with context
    pub fn browser(context: &'static str, err: impl std::fmt::Display) -> Self {
        Self::Browser {
            context,
            message: err.to_string(),
        }
    }

    /// Create a navigation error with URL context
    pub fn navigation(url: impl Into<String>, err: impl std::fmt::Display) -> Self {
        Self::NavigationFailed {
            url: url.into(),
            message: err.to_string(),
        }
    }

    /// Create an API error with status code
    pub fn api(status_code: u16, message: impl Into<String>) -> Self {
        Self::ApiError {
            status_code,
            message: message.into(),
        }
    }

    /// Get the appropriate HTTP status code for this error
    pub fn http_status_code(&self) -> u16 {
        match self {
            // Client errors (4xx)
            Self::LoginFailed(_) => 401,  // Unauthorized
            Self::SessionError(_) => 401, // Unauthorized
            Self::NavigationFailed { .. } => 400, // Bad Request (invalid navigation state)

            // Server errors (5xx)
            Self::Browser { .. } => 500,       // Internal Server Error
            Self::ExtractionFailed(_) => 500,  // Internal Server Error
            Self::Storage(_) => 500,           // Internal Server Error
            Self::HttpRequest(_) => 502,       // Bad Gateway (upstream error)
            Self::JsonError(_) => 500,         // Internal Server Error

            // API errors - use the original status code if available
            Self::ApiError { status_code, .. } => *status_code,

            // gRPC errors are mapped to 502 (Bad Gateway - upstream error)
            Self::GrpcError(_) => 502,
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::HttpRequest(_) => true,  // Network errors are often transient
            Self::ApiError { status_code, .. } => {
                // 5xx errors and some 4xx are retryable
                *status_code >= 500 || *status_code == 429
            }
            Self::Browser { .. } => true,  // Browser errors might be transient
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, RendererError>;

/// Vehicle data extracted from the website
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VehicleData {
    #[serde(rename = "VehicleCD")]
    pub vehicle_cd: String,
    #[serde(rename = "VehicleName")]
    pub vehicle_name: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Metadata")]
    pub metadata: HashMap<String, String>,
}

/// Response from Hono API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HonoApiResponse {
    pub success: bool,
    pub records_added: i32,
    pub total_records: i32,
    pub message: String,
}

/// Browser renderer for vehicle data extraction
pub struct Renderer {
    config: Arc<Config>,
    storage: Arc<Storage>,
    browser: Browser,
    http_client: Client,
}

impl Renderer {
    /// Create a new renderer instance
    pub async fn new(config: Arc<Config>, storage: Arc<Storage>) -> Result<Self> {
        info!("Initializing browser renderer...");

        // Generate unique user data directory to avoid conflicts between instances
        // Use process ID + nanosecond timestamp for uniqueness
        let unique_id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let user_data_dir = std::env::temp_dir()
            .join(format!("chromiumoxide-{}", unique_id));

        // Get Chrome path from environment or use default
        let chrome_path = std::env::var("CHROME_PATH")
            .or_else(|_| std::env::var("CHROMIUM_PATH"))
            .unwrap_or_else(|_| "chromium".to_string());

        // Build browser configuration
        let mut builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(&user_data_dir);

        // with_head() enables GUI mode, so we call it only when NOT headless
        if !config.browser_headless {
            builder = builder.with_head();
        }

        builder = builder
            .no_sandbox()
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu");

        if config.browser_debug {
            builder = builder
                .arg("--enable-logging=stderr")
                .arg("--v=1");
        }

        let browser_config = builder
            .build()
            .map_err(|e| RendererError::browser("config build", e))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| RendererError::browser("launch", e))?;

        // Spawn handler task
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Browser event: {:?}", event);
            }
        });

        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| RendererError::browser("http client build", e))?;

        info!("Browser renderer initialized successfully");

        Ok(Renderer {
            config,
            storage,
            browser,
            http_client,
        })
    }

    /// Get vehicle data from the website
    pub async fn get_vehicle_data(
        &self,
        session_id: &str,
        branch_id: &str,
        filter_id: &str,
        force_login: bool,
    ) -> Result<(Vec<VehicleData>, String, Option<HonoApiResponse>)> {
        info!("GetVehicleData called");

        // Use fixed parameters for initial request
        let branch_id = "00000000";
        let filter_id = "0";
        info!(
            "Using fixed parameters - BranchID: {}, FilterID: {}, ForceLogin: {}",
            branch_id, filter_id, force_login
        );

        // Create new page
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| RendererError::browser("new page", e))?;

        let mut current_session_id = session_id.to_string();

        // Restore session cookies if exists
        if !session_id.is_empty() && !force_login {
            if let Ok(Some(_session)) = self.storage.get_session(session_id).await {
                if let Ok(cookies) = self.storage.get_cookies(session_id).await {
                    for cookie in cookies {
                        let cookie_param = CookieParam::builder()
                            .name(&cookie.name)
                            .value(&cookie.value)
                            .domain(&cookie.domain)
                            .path(&cookie.path)
                            .http_only(cookie.http_only)
                            .secure(cookie.secure)
                            .build();

                        if let Ok(param) = cookie_param {
                            if let Err(e) = page.set_cookie(param).await {
                                debug!("Failed to set cookie: {}", e);
                            }
                        }
                    }
                }
            }
        }

        // Try to navigate to main page
        match self.navigate_to_main(&page, branch_id, filter_id).await {
            Ok(_) => {
                info!("Navigation to main page successful without login");
            }
            Err(e) => {
                info!("First navigation failed, attempting login: {}", e);

                // Need to login
                current_session_id = self.login(&page).await?;
                info!("Login successful, new session ID: {}", current_session_id);

                // Navigate again after login
                self.navigate_to_main(&page, branch_id, filter_id).await?;
                info!("Navigation to main page successful after login");
            }
        }

        // Extract vehicle data
        let (vehicles, raw_data) = self.extract_vehicle_data(&page, branch_id, filter_id).await?;

        // Cache the data (non-critical, log errors but continue)
        for vehicle in &vehicles {
            if let Err(e) = self
                .storage
                .cache_vehicle_data(&vehicle.vehicle_cd, vehicle, Duration::from_secs(300))
                .await
            {
                debug!("Failed to cache vehicle data for {}: {}", vehicle.vehicle_cd, e);
            }
        }

        // Send to rust-logi via gRPC (requires grpc feature)
        #[cfg(feature = "grpc")]
        let hono_response = match self.send_to_rust_logi(&raw_data).await {
            Ok(resp) => Some(resp),
            Err(e) => {
                warn!("Failed to send to rust-logi: {}", e);
                None
            }
        };

        #[cfg(not(feature = "grpc"))]
        let hono_response: Option<HonoApiResponse> = {
            warn!("grpc feature not enabled - data not sent to rust-logi");
            None
        };

        // Close page (best effort, don't fail the operation if close fails)
        if let Err(e) = page.close().await {
            debug!("Failed to close page: {}", e);
        }

        Ok((vehicles, current_session_id, hono_response))
    }

    /// Perform login
    async fn login(&self, page: &Page) -> Result<String> {
        info!("Starting login process");
        info!(
            "Using credentials - Company: {}, User: {}",
            self.config.comp_id, self.config.user_name
        );

        // Navigate to login page
        let login_url = "https://theearth-np.com/F-OES1010[Login].aspx?mode=timeout";
        page.goto(login_url)
            .await
            .map_err(|e| RendererError::navigation(login_url, e))?;

        // Wait for page to load
        sleep(Duration::from_secs(3)).await;

        // Check if login form exists
        let has_pass_field = page
            .evaluate("document.querySelector('#txtPass') !== null")
            .await
            .map_err(|e| RendererError::browser("check login form", e))?;

        if !has_pass_field.into_value::<bool>().unwrap_or(false) {
            return Err(RendererError::LoginFailed("Login form not found".to_string()));
        }

        // Handle popup if present (non-critical, continue even if this fails)
        if let Err(e) = page
            .evaluate(
                r#"
                const popup = document.querySelector('#popup_1');
                if (popup && popup.style.display !== 'none') {
                    popup.click();
                }
            "#,
            )
            .await
        {
            debug!("Failed to handle popup (may not exist): {}", e);
        }

        sleep(Duration::from_secs(1)).await;

        // Fill credentials
        let fill_script = format!(
            r#"
            document.querySelector('#txtID2').value = '{}';
            document.querySelector('#txtID1').value = '{}';
            document.querySelector('#txtPass').value = '{}';
        "#,
            self.config.comp_id, self.config.user_name, self.config.user_pass
        );

        page.evaluate(fill_script.as_str())
            .await
            .map_err(|e| RendererError::browser("fill credentials", e))?;

        // Take debug screenshot if enabled
        if self.config.browser_debug {
            if let Ok(screenshot) = page
                .screenshot(ScreenshotParams::builder().full_page(true).build())
                .await
            {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&screenshot);
                debug!("Login screenshot: data:image/png;base64,{}", encoded);
            }
        }

        // Click login button
        page.evaluate("document.querySelector('#imgLogin').click()")
            .await
            .map_err(|e| RendererError::browser("click login button", e))?;

        // Wait for navigation
        sleep(Duration::from_secs(5)).await;

        // Check if login was successful
        let login_success = page
            .evaluate("document.querySelector('#Button1st_7') !== null")
            .await
            .map_err(|e| RendererError::browser("check login success", e))?;

        if !login_success.into_value::<bool>().unwrap_or(false) {
            // Handle case where user is already logged in
            let has_popup = page
                .evaluate("document.querySelector('#popup_1') !== null")
                .await
                .map_err(|e| RendererError::browser("check popup", e))?;

            if has_popup.into_value::<bool>().unwrap_or(false) {
                page.evaluate("document.querySelector('#popup_1').click()")
                    .await
                    .map_err(|e| RendererError::browser("click popup", e))?;

                sleep(Duration::from_secs(5)).await;
            } else {
                return Err(RendererError::LoginFailed(
                    "Login verification failed".to_string(),
                ));
            }
        }

        // Create new session
        let session_id = format!("session_{}", Utc::now().timestamp());
        let now = Utc::now();
        let session = Session {
            id: session_id.clone(),
            created_at: now,
            updated_at: now,
            expires_at: now + chrono::Duration::from_std(self.config.session_ttl).unwrap_or_default(),
            user_id: self.config.user_name.clone(),
            company_id: self.config.comp_id.clone(),
        };

        if let Err(e) = self.storage.create_session(&session).await {
            error!("Failed to save session: {}", e);
        }

        // Save cookies
        if let Ok(browser_cookies) = page.get_cookies().await {
            let cookies: Vec<Cookie> = browser_cookies
                .into_iter()
                .map(|c| Cookie {
                    name: c.name,
                    value: c.value,
                    domain: c.domain,
                    path: c.path,
                    expires_at: Utc::now() + chrono::Duration::hours(24),
                    http_only: c.http_only,
                    secure: c.secure,
                })
                .collect();

            if let Err(e) = self.storage.save_cookies(&session_id, &cookies).await {
                error!("Failed to save cookies: {}", e);
            }
        }

        info!("Login successful, session ID: {}", session_id);
        Ok(session_id)
    }

    /// Navigate to Venus Main page
    async fn navigate_to_main(&self, page: &Page, _branch_id: &str, _filter_id: &str) -> Result<()> {
        info!("Navigating to Venus Main page...");

        let main_url = "https://theearth-np.com/WebVenus/F-AAV0001[VenusMain].aspx";
        page.goto(main_url)
            .await
            .map_err(|e| RendererError::navigation(main_url, e))?;

        // Wait for page to load
        sleep(Duration::from_secs(5)).await;

        // Check if we're still on the login page
        let current_url = page
            .evaluate("window.location.href")
            .await
            .map_err(|e| RendererError::browser("get current URL", e))?;

        let url = current_url.into_value::<String>().unwrap_or_default();
        info!("Current URL after navigation: {}", url);

        if url.contains("Login") || url.contains("OES1010") {
            return Err(RendererError::NavigationFailed {
                url: main_url.to_string(),
                message: "Redirected to login page - session may have expired".to_string(),
            });
        }

        Ok(())
    }

    /// Extract vehicle data from the page
    async fn extract_vehicle_data(
        &self,
        page: &Page,
        branch_id: &str,
        filter_id: &str,
    ) -> Result<(Vec<VehicleData>, Vec<serde_json::Value>)> {
        let filter_id = if filter_id.is_empty() { "0" } else { filter_id };

        // Check if VenusBridgeService exists
        let has_service = page
            .evaluate(
                r#"
                typeof VenusBridgeService !== 'undefined' &&
                typeof VenusBridgeService.VehicleStateTableForBranchEx === 'function'
            "#,
            )
            .await
            .map_err(|e| RendererError::browser("check VenusBridgeService", e))?;

        if !has_service.into_value::<bool>().unwrap_or(false) {
            return Err(RendererError::ExtractionFailed(
                "VenusBridgeService not found on page".to_string(),
            ));
        }

        info!(
            "Calling VenusBridgeService.VehicleStateTableForBranchEx with branchID='{}', filterID='{}'",
            branch_id, filter_id
        );

        // Wait for page to stabilize
        sleep(Duration::from_secs(2)).await;

        // Wait for grid to appear
        info!("Waiting for page to be ready...");
        for i in 0..30 {
            let grid_exists = page
                .evaluate("document.querySelector('#igGrid-VenusMain-VehicleList') !== null")
                .await
                .map_err(|e| RendererError::browser("check grid exists", e))?;

            if grid_exists.into_value::<bool>().unwrap_or(false) {
                info!("Venus main grid detected, page structure loaded");
                break;
            }

            if i % 5 == 0 {
                info!("Waiting for page structure... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        // Wait for loading indicators to disappear
        info!("Checking for loading messages...");
        for i in 0..30 {
            let has_loading = page
                .evaluate(
                    r#"
                    (() => {
                        const waitMsg = document.querySelector('#pMsg_wait, [id*="pMsg_wait"]');
                        if (!waitMsg) return false;
                        const style = window.getComputedStyle(waitMsg);
                        return style.display !== 'none' && style.visibility !== 'hidden';
                    })()
                "#,
                )
                .await
                .map_err(|e| RendererError::browser("check loading messages", e))?;

            if !has_loading.into_value::<bool>().unwrap_or(false) {
                info!("No loading messages detected, proceeding...");
                break;
            }

            if i % 5 == 0 {
                info!("Loading message still visible, waiting... ({}/30)", i + 1);
            }
            sleep(Duration::from_secs(1)).await;
        }

        // Additional wait
        sleep(Duration::from_secs(3)).await;

        // Execute JavaScript to get vehicle data
        info!("Executing JavaScript to get vehicle data...");

        let inject_script = format!(
            r#"
            window.__vehicleDataResult = null;
            window.__vehicleDataError = null;
            window.__vehicleDataCompleted = false;

            VenusBridgeService.VehicleStateTableForBranchEx('{}', '{}',
                (data) => {{
                    window.__vehicleDataResult = data;
                    window.__vehicleDataCompleted = true;
                }},
                (error) => {{
                    window.__vehicleDataError = error;
                    window.__vehicleDataCompleted = true;
                }}
            );
        "#,
            branch_id, filter_id
        );

        page.evaluate(inject_script.as_str())
            .await
            .map_err(|e| RendererError::browser("inject vehicle data script", e))?;

        // Poll for result
        info!("Waiting for vehicle data response...");
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(60);

        loop {
            if start.elapsed() > timeout {
                return Err(RendererError::ExtractionFailed(format!(
                    "Timeout waiting for vehicle data after {:?}",
                    timeout
                )));
            }

            let completed = page
                .evaluate("window.__vehicleDataCompleted")
                .await
                .map_err(|e| RendererError::browser("poll data completion", e))?;

            if completed.into_value::<bool>().unwrap_or(false) {
                // Check for error
                let has_error = page
                    .evaluate("window.__vehicleDataError !== null")
                    .await
                    .map_err(|e| RendererError::browser("check data error", e))?;

                if has_error.into_value::<bool>().unwrap_or(false) {
                    let error_msg = page
                        .evaluate("window.__vehicleDataError")
                        .await
                        .map_err(|e| RendererError::browser("get error message", e))?;

                    return Err(RendererError::ExtractionFailed(format!(
                        "Service error: {:?}",
                        error_msg.into_value::<String>()
                    )));
                }

                // Get result
                let result = page
                    .evaluate("JSON.stringify(window.__vehicleDataResult)")
                    .await
                    .map_err(|e| RendererError::browser("get vehicle data result", e))?;

                let json_str = result.into_value::<String>().unwrap_or_default();
                info!(
                    "Got vehicle data response after {:?}",
                    start.elapsed()
                );

                // Parse JSON
                let raw_data: Vec<serde_json::Value> = serde_json::from_str(&json_str)
                    .map_err(|e| RendererError::ExtractionFailed(e.to_string()))?;

                // Convert to VehicleData
                let vehicles = self.parse_vehicle_data(&raw_data);
                info!("Extracted {} vehicles", vehicles.len());

                // Save raw data to file
                self.save_raw_data(&raw_data).await;

                return Ok((vehicles, raw_data));
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    /// Parse raw JSON data to VehicleData structs
    fn parse_vehicle_data(&self, raw_data: &[serde_json::Value]) -> Vec<VehicleData> {
        raw_data
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;

                let vehicle_cd = obj
                    .get("VehicleCD")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let vehicle_name = obj
                    .get("VehicleName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let status = obj
                    .get("Status")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                let mut metadata = HashMap::new();
                for (k, v) in obj {
                    if k != "VehicleCD" && k != "VehicleName" && k != "Status" {
                        metadata.insert(k.clone(), format!("{}", v));
                    }
                }

                Some(VehicleData {
                    vehicle_cd,
                    vehicle_name,
                    status,
                    metadata,
                })
            })
            .collect()
    }

    /// Save raw data to JSON file
    async fn save_raw_data(&self, raw_data: &[serde_json::Value]) {
        let jst = FixedOffset::east_opt(9 * 3600).unwrap(); // JST = UTC+9
        let timestamp = Utc::now().with_timezone(&jst).format("%Y%m%d_%H%M%S");
        let filename = format!("./data/vehicles_{}.json", timestamp);

        // Create data directory (non-critical, logging handled later if file write fails)
        if let Err(e) = std::fs::create_dir_all("./data") {
            warn!("Failed to create data directory: {}", e);
            return;
        }

        match serde_json::to_string_pretty(raw_data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&filename, json) {
                    error!("Failed to save vehicle data: {}", e);
                } else {
                    info!("Saved vehicle data to {}", filename);
                }
            }
            Err(e) => error!("Failed to serialize vehicle data: {}", e),
        }
    }

    /// Send raw data to rust-logi via gRPC
    #[cfg(feature = "grpc")]
    async fn send_to_rust_logi(
        &self,
        raw_data: &[serde_json::Value],
    ) -> Result<HonoApiResponse> {
        use crate::logi::dtakologs::dtakologs_service_client::DtakologsServiceClient;
        use crate::logi::dtakologs::{BulkCreateDtakologsRequest, Dtakolog};
        use tonic::metadata::MetadataValue;
        use tonic::Request;

        // Check if rust-logi URL is configured
        if self.config.rust_logi_url.is_empty() {
            return Err(RendererError::GrpcError(
                "RUST_LOGI_URL not configured".to_string(),
            ));
        }

        info!(
            "send_to_rust_logi: Sending {} records to {}",
            raw_data.len(),
            self.config.rust_logi_url
        );

        // Create gRPC client
        let channel = tonic::transport::Channel::from_shared(self.config.rust_logi_url.clone())
            .map_err(|e| RendererError::GrpcError(format!("Invalid URL: {}", e)))?
            .connect()
            .await
            .map_err(|e| RendererError::GrpcError(format!("Connection failed: {}", e)))?;

        let mut client = DtakologsServiceClient::new(channel);

        // Convert raw data to Dtakolog messages
        let dtakologs: Vec<Dtakolog> = raw_data
            .iter()
            .filter_map(|v| {
                // Parse JSON to Dtakolog
                let obj = v.as_object()?;
                Some(Dtakolog {
                    r#type: obj.get("__type")?.as_str()?.to_string(),
                    address_disp_c: obj.get("AddressDispC").and_then(|v| v.as_str()).map(String::from),
                    address_disp_p: obj.get("AddressDispP").and_then(|v| v.as_str()).map(String::from),
                    all_state: obj.get("AllState").and_then(|v| v.as_str()).map(String::from),
                    all_state_ex: obj.get("AllStateEx").and_then(|v| v.as_str()).map(String::from),
                    all_state_font_color: obj.get("AllStateFontColor").and_then(|v| v.as_str()).map(String::from),
                    all_state_font_color_index: obj.get("AllStateFontColorIndex").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    all_state_ryout_color: obj.get("AllStateRyoutColor").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    branch_cd: obj.get("BranchCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    branch_name: obj.get("BranchName").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    comu_date_time: obj.get("ComuDateTime").and_then(|v| v.as_str()).map(String::from),
                    current_work_cd: obj.get("CurrentWorkCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    current_work_name: obj.get("CurrentWorkName").and_then(|v| v.as_str()).map(String::from),
                    data_date_time: self.convert_data_date_time(obj.get("DataDateTime").and_then(|v| v.as_str()).unwrap_or("")),
                    data_filter_type: obj.get("DataFilterType").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    disp_flag: obj.get("DispFlag").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    driver_cd: obj.get("DriverCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    driver_name: obj.get("DriverName").and_then(|v| v.as_str()).map(String::from),
                    event_val: obj.get("EventVal").and_then(|v| v.as_str()).map(String::from),
                    gps_direction: obj.get("GPSDirection").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    gps_enable: obj.get("GPSEnable").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    gps_lati_and_long: obj.get("GPSLatiAndLong").and_then(|v| v.as_str()).map(String::from),
                    gps_latitude: obj.get("GPSLatitude").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    gps_longitude: obj.get("GPSLongitude").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    gps_satellite_num: obj.get("GPSSatelliteNum").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    odometer: obj.get("ODOMeter").and_then(|v| v.as_str()).map(String::from),
                    operation_state: obj.get("OperationState").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    recive_event_type: obj.get("ReciveEventType").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    recive_packet_type: obj.get("RecivePacketType").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    recive_type_color_name: obj.get("ReciveTypeColorName").and_then(|v| v.as_str()).map(String::from),
                    recive_type_name: obj.get("ReciveTypeName").and_then(|v| v.as_str()).map(String::from),
                    recive_work_cd: obj.get("ReciveWorkCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    revo: obj.get("Revo").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    setting_temp: obj.get("SettingTemp").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    setting_temp1: obj.get("SettingTemp1").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    setting_temp3: obj.get("SettingTemp3").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    setting_temp4: obj.get("SettingTemp4").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    speed: obj.get("Speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    start_work_date_time: obj.get("StartWorkDateTime").and_then(|v| v.as_str()).map(String::from),
                    state: obj.get("State").and_then(|v| v.as_str()).map(String::from),
                    state1: obj.get("State1").and_then(|v| v.as_str()).map(String::from),
                    state2: obj.get("State2").and_then(|v| v.as_str()).map(String::from),
                    state3: obj.get("State3").and_then(|v| v.as_str()).map(String::from),
                    state_flag: obj.get("StateFlag").and_then(|v| v.as_str()).map(String::from),
                    sub_driver_cd: obj.get("SubDriverCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    temp1: obj.get("Temp1").and_then(|v| v.as_str()).map(String::from),
                    temp2: obj.get("Temp2").and_then(|v| v.as_str()).map(String::from),
                    temp3: obj.get("Temp3").and_then(|v| v.as_str()).map(String::from),
                    temp4: obj.get("Temp4").and_then(|v| v.as_str()).map(String::from),
                    temp_state: obj.get("TempState").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    vehicle_cd: obj.get("VehicleCD").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    vehicle_icon_color: obj.get("VehicleIconColor").and_then(|v| v.as_str()).map(String::from),
                    vehicle_icon_label_for_datetime: obj.get("VehicleIconLabelForDatetime").and_then(|v| v.as_str()).map(String::from),
                    vehicle_icon_label_for_driver: obj.get("VehicleIconLabelForDriver").and_then(|v| v.as_str()).map(String::from),
                    vehicle_icon_label_for_vehicle: obj.get("VehicleIconLabelForVehicle").and_then(|v| v.as_str()).map(String::from),
                    vehicle_name: obj.get("VehicleName").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
            })
            .collect();

        info!("Converted {} records to Dtakolog messages", dtakologs.len());

        // Create request with organization ID header
        let mut request = Request::new(BulkCreateDtakologsRequest { dtakologs });
        if !self.config.rust_logi_organization_id.is_empty() {
            request.metadata_mut().insert(
                "x-organization-id",
                MetadataValue::try_from(&self.config.rust_logi_organization_id)
                    .map_err(|e| RendererError::GrpcError(format!("Invalid organization ID: {}", e)))?,
            );
        }

        // Send request
        let response = client
            .bulk_create(request)
            .await
            .map_err(|e| RendererError::GrpcError(format!("BulkCreate failed: {}", e)))?;

        let resp = response.into_inner();
        info!(
            "rust-logi response: success={}, records_added={}, total_records={}, message={}",
            resp.success, resp.records_added, resp.total_records, resp.message
        );

        Ok(HonoApiResponse {
            success: resp.success,
            records_added: resp.records_added,
            total_records: resp.total_records,
            message: resp.message,
        })
    }

    /// Convert DataDateTime from "YY/MM/DD HH:MM" to ISO8601 format
    fn convert_data_date_time(&self, date_str: &str) -> String {
        if date_str.is_empty() {
            return "2020-01-01T00:00:00+09:00".to_string();
        }
        // Convert "24/11/28 10:37" to "2024-11-28T10:37:00+09:00"
        let full_date = format!("20{}", date_str);
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&full_date, "%Y/%m/%d %H:%M") {
            let jst = FixedOffset::east_opt(9 * 3600).unwrap();
            return dt.and_local_timezone(jst).unwrap().to_rfc3339();
        }
        // Fallback
        format!("20{}", date_str)
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

    /// Extract vehicle code from vehicle name
    #[allow(dead_code)]
    fn extract_vehicle_code(&self, vehicle_name: &str) -> i32 {
        if vehicle_name.is_empty() {
            return 0;
        }

        let re = Regex::new(r"\d+").unwrap();
        let matches: Vec<&str> = re.find_iter(vehicle_name).map(|m| m.as_str()).collect();

        if !matches.is_empty() {
            let combined: String = matches.join("");
            if let Ok(code) = combined.parse::<i64>() {
                return (code % 2147483647) as i32;
            }
        }

        // Generate hash-based ID
        let hash: i32 = vehicle_name
            .chars()
            .fold(0i32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as i32));
        hash.abs() % 2147483647
    }

    /// Close the browser
    pub async fn close(&self) -> Result<()> {
        // Browser will be closed when dropped
        Ok(())
    }
}
