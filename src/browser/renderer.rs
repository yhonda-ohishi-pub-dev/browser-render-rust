use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
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
    #[error("Browser error: {0}")]
    Browser(String),
    #[error("Login failed: {0}")]
    LoginFailed(String),
    #[error("Navigation failed: {0}")]
    NavigationFailed(String),
    #[error("Data extraction failed: {0}")]
    ExtractionFailed(String),
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Session error: {0}")]
    SessionError(String),
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),
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

        // Build browser configuration
        let mut builder = BrowserConfig::builder()
            .user_data_dir(&user_data_dir);

        // with_head() enables GUI mode, so we call it only when NOT headless
        if !config.browser_headless {
            builder = builder.with_head();
        }

        builder = builder
            .no_sandbox()
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-dev-shm-usage");

        if config.browser_debug {
            builder = builder
                .arg("--enable-logging=stderr")
                .arg("--v=1");
        }

        let browser_config = builder
            .build()
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        // Spawn handler task
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Browser event: {:?}", event);
            }
        });

        let http_client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| RendererError::Browser(e.to_string()))?;

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
            .map_err(|e| RendererError::Browser(e.to_string()))?;

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
                            let _ = page.set_cookie(param).await;
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

        // Cache the data
        for vehicle in &vehicles {
            let _ = self
                .storage
                .cache_vehicle_data(&vehicle.vehicle_cd, vehicle, Duration::from_secs(300))
                .await;
        }

        // Send to Hono API
        let hono_response = match self.send_raw_to_hono_api(&raw_data).await {
            Ok(resp) => Some(resp),
            Err(e) => {
                warn!("Failed to send to Hono API: {}", e);
                None
            }
        };

        // Close page
        let _ = page.close().await;

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
        page.goto("https://theearth-np.com/F-OES1010[Login].aspx?mode=timeout")
            .await
            .map_err(|e| RendererError::NavigationFailed(e.to_string()))?;

        // Wait for page to load
        sleep(Duration::from_secs(3)).await;

        // Check if login form exists
        let has_pass_field = page
            .evaluate("document.querySelector('#txtPass') !== null")
            .await
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        if !has_pass_field.into_value::<bool>().unwrap_or(false) {
            return Err(RendererError::LoginFailed("Login form not found".to_string()));
        }

        // Handle popup if present
        let _ = page
            .evaluate(
                r#"
                const popup = document.querySelector('#popup_1');
                if (popup && popup.style.display !== 'none') {
                    popup.click();
                }
            "#,
            )
            .await;

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
            .map_err(|e| RendererError::Browser(e.to_string()))?;

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
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        // Wait for navigation
        sleep(Duration::from_secs(5)).await;

        // Check if login was successful
        let login_success = page
            .evaluate("document.querySelector('#Button1st_7') !== null")
            .await
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        if !login_success.into_value::<bool>().unwrap_or(false) {
            // Handle case where user is already logged in
            let has_popup = page
                .evaluate("document.querySelector('#popup_1') !== null")
                .await
                .map_err(|e| RendererError::Browser(e.to_string()))?;

            if has_popup.into_value::<bool>().unwrap_or(false) {
                page.evaluate("document.querySelector('#popup_1').click()")
                    .await
                    .map_err(|e| RendererError::Browser(e.to_string()))?;

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

        page.goto("https://theearth-np.com/WebVenus/F-AAV0001[VenusMain].aspx")
            .await
            .map_err(|e| RendererError::NavigationFailed(e.to_string()))?;

        // Wait for page to load
        sleep(Duration::from_secs(5)).await;

        // Check if we're still on the login page
        let current_url = page
            .evaluate("window.location.href")
            .await
            .map_err(|e| RendererError::Browser(e.to_string()))?;

        let url = current_url.into_value::<String>().unwrap_or_default();
        info!("Current URL after navigation: {}", url);

        if url.contains("Login") || url.contains("OES1010") {
            return Err(RendererError::NavigationFailed(
                "Redirected to login page".to_string(),
            ));
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
            .map_err(|e| RendererError::Browser(e.to_string()))?;

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
                .map_err(|e| RendererError::Browser(e.to_string()))?;

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
                .map_err(|e| RendererError::Browser(e.to_string()))?;

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
            .map_err(|e| RendererError::Browser(e.to_string()))?;

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
                .map_err(|e| RendererError::Browser(e.to_string()))?;

            if completed.into_value::<bool>().unwrap_or(false) {
                // Check for error
                let has_error = page
                    .evaluate("window.__vehicleDataError !== null")
                    .await
                    .map_err(|e| RendererError::Browser(e.to_string()))?;

                if has_error.into_value::<bool>().unwrap_or(false) {
                    let error_msg = page
                        .evaluate("window.__vehicleDataError")
                        .await
                        .map_err(|e| RendererError::Browser(e.to_string()))?;

                    return Err(RendererError::ExtractionFailed(format!(
                        "Service error: {:?}",
                        error_msg.into_value::<String>()
                    )));
                }

                // Get result
                let result = page
                    .evaluate("JSON.stringify(window.__vehicleDataResult)")
                    .await
                    .map_err(|e| RendererError::Browser(e.to_string()))?;

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
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("./data/vehicles_{}.json", timestamp);

        // Create data directory
        let _ = std::fs::create_dir_all("./data");

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

    /// Send raw data to Hono API
    async fn send_raw_to_hono_api(
        &self,
        raw_data: &[serde_json::Value],
    ) -> Result<HonoApiResponse> {
        info!("sendRawToHonoAPI: Sending {} records", raw_data.len());

        let json_data =
            serde_json::to_string(raw_data).map_err(|e| RendererError::ApiError(e.to_string()))?;

        info!("sendRawToHonoAPI: JSON size: {} bytes", json_data.len());

        let response = self
            .http_client
            .post(&self.config.hono_api_url)
            .header("Content-Type", "application/json; charset=utf-8")
            .body(json_data)
            .send()
            .await
            .map_err(|e| RendererError::ApiError(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| RendererError::ApiError(e.to_string()))?;

        if !status.is_success() {
            error!("API Error - Status: {}, Body: {}", status, body);
            return Err(RendererError::ApiError(format!(
                "API returned status {}",
                status
            )));
        }

        info!("API Success - Status: {}, Body: {}", status, body);

        Ok(HonoApiResponse {
            success: true,
            records_added: raw_data.len() as i32,
            total_records: raw_data.len() as i32,
            message: format!("Successfully sent {} records to Hono API", raw_data.len()),
        })
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
