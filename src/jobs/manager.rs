use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc, offset::FixedOffset};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::browser::HonoApiResponse;
use crate::config::Config;

/// Job type enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobType {
    Vehicle,
    EtcScrape,
    EtcScrapeBatch,
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobType::Vehicle => write!(f, "vehicle"),
            JobType::EtcScrape => write!(f, "etc_scrape"),
            JobType::EtcScrapeBatch => write!(f, "etc_scrape_batch"),
        }
    }
}

/// Job priority enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum JobPriority {
    Low,      // Run when idle
    Normal,   // Normal priority
    High,     // Run immediately
}

impl Default for JobPriority {
    fn default() -> Self {
        JobPriority::Normal
    }
}

/// Job status enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Queued,
    Running,
    Completed,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Queued => write!(f, "queued"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

/// ETC scrape request data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcScrapeRequest {
    pub user_id: String,
    pub password: String,
    #[serde(default = "default_download_path")]
    pub download_path: String,
    #[serde(default = "default_headless")]
    pub headless: bool,
}

fn default_download_path() -> String {
    std::env::var("ETC_DOWNLOAD_PATH").unwrap_or_else(|_| "./downloads".to_string())
}

fn default_headless() -> bool {
    true
}

fn default_download_timeout() -> Duration {
    std::env::var("ETC_DOWNLOAD_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(120))
}

/// ETC scrape result data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcScrapeResult {
    pub csv_path: String,
    pub csv_size: usize,
}

/// Account info for batch request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcAccountInfo {
    pub user_id: String,
    pub password: String,
    #[serde(default)]
    pub name: String,
}

/// ETC batch scrape request data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcBatchRequest {
    pub accounts: Vec<EtcAccountInfo>,
    #[serde(default = "default_download_path")]
    pub download_path: String,
    #[serde(default = "default_headless")]
    pub headless: bool,
}

/// Result for a single account in a batch job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountResult {
    pub user_id: String,
    pub name: String,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AccountResult {
    pub fn new(user_id: String, name: String) -> Self {
        Self {
            user_id,
            name,
            status: JobStatus::Pending,
            csv_path: None,
            csv_size: None,
            error: None,
        }
    }

    pub fn set_running(&mut self) {
        self.status = JobStatus::Running;
    }

    pub fn set_completed(&mut self, csv_path: String, csv_size: usize) {
        self.status = JobStatus::Completed;
        self.csv_path = Some(csv_path);
        self.csv_size = Some(csv_size);
    }

    pub fn set_failed(&mut self, error: String) {
        self.status = JobStatus::Failed;
        self.error = Some(error);
    }
}

/// ETC batch scrape result data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcBatchResult {
    pub session_folder: String,
    pub accounts: Vec<AccountResult>,
    pub total_count: usize,
    pub success_count: usize,
    pub fail_count: usize,
}

/// Job data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub job_type: JobType,
    pub priority: JobPriority,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    // Vehicle job specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hono_response: Option<HonoApiResponse>,
    // ETC scrape job specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etc_request: Option<EtcScrapeRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etc_result: Option<EtcScrapeResult>,
    // ETC batch scrape job specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_request: Option<EtcBatchRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_result: Option<EtcBatchResult>,
    /// Current account index (for batch jobs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_account_index: Option<usize>,
}

/// Queued job entry
#[derive(Debug, Clone)]
struct QueuedJob {
    id: String,
    #[allow(dead_code)]
    priority: JobPriority,
}

/// Job manager for handling background jobs
pub struct JobManager {
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    queue: Arc<RwLock<VecDeque<QueuedJob>>>,
    config: Arc<Config>,
    running_count: Arc<RwLock<usize>>,
    /// Semaphore to ensure only one vehicle job runs at a time
    vehicle_semaphore: Arc<Semaphore>,
}

impl JobManager {
    /// Create a new job manager
    pub fn new(config: Arc<Config>) -> Self {
        JobManager {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            queue: Arc::new(RwLock::new(VecDeque::new())),
            config,
            running_count: Arc::new(RwLock::new(0)),
            // Only allow 1 vehicle job at a time to prevent session conflicts
            vehicle_semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    /// Format current timestamp in JST (Japan Standard Time, UTC+9)
    fn format_jst_timestamp() -> String {
        let jst = FixedOffset::east_opt(9 * 3600).unwrap(); // JST = UTC+9
        Utc::now().with_timezone(&jst).format("%Y%m%d_%H%M%S").to_string()
    }

    /// Check if the system is idle (no jobs running)
    pub async fn is_idle(&self) -> bool {
        let count = self.running_count.read().await;
        *count == 0
    }

    /// Get the count of running jobs
    pub async fn running_job_count(&self) -> usize {
        let count = self.running_count.read().await;
        *count
    }

    /// Create a new vehicle job and start processing in background
    pub async fn create_vehicle_job(&self) -> String {
        let job_id = Uuid::new_v4().to_string();

        let job = Job {
            id: job_id.clone(),
            job_type: JobType::Vehicle,
            priority: JobPriority::Normal,
            status: JobStatus::Pending,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            vehicle_count: None,
            hono_response: None,
            etc_request: None,
            etc_result: None,
            batch_request: None,
            batch_result: None,
            current_account_index: None,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }

        // Start processing in background with semaphore to ensure serial execution
        let jobs = self.jobs.clone();
        let config = self.config.clone();
        let running_count = self.running_count.clone();
        let job_id_clone = job_id.clone();
        let semaphore = self.vehicle_semaphore.clone();

        tokio::spawn(async move {
            // Acquire semaphore permit - only one vehicle job can run at a time
            let _permit = semaphore.acquire().await.expect("Semaphore closed");
            info!("Vehicle job {} acquired semaphore, starting execution", job_id_clone);
            process_vehicle_job(jobs, config, running_count, job_id_clone).await;
            // Permit is automatically released when _permit is dropped
        });

        job_id
    }

    /// Create a new job (legacy alias for create_vehicle_job)
    pub async fn create_job(&self) -> String {
        self.create_vehicle_job().await
    }

    /// Create a new ETC scrape job
    pub async fn create_etc_job(&self, request: EtcScrapeRequest, priority: JobPriority) -> String {
        let job_id = Uuid::new_v4().to_string();

        let job = Job {
            id: job_id.clone(),
            job_type: JobType::EtcScrape,
            priority: priority.clone(),
            status: if priority == JobPriority::Low {
                JobStatus::Queued
            } else {
                JobStatus::Pending
            },
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            vehicle_count: None,
            hono_response: None,
            etc_request: Some(request.clone()),
            etc_result: None,
            batch_request: None,
            batch_result: None,
            current_account_index: None,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }

        match priority {
            JobPriority::Low => {
                // Add to queue, will be processed when idle
                let mut queue = self.queue.write().await;
                queue.push_back(QueuedJob {
                    id: job_id.clone(),
                    priority: priority.clone(),
                });
                info!("ETC job {} added to queue (low priority)", job_id);
            }
            _ => {
                // Start processing immediately
                let jobs = self.jobs.clone();
                let running_count = self.running_count.clone();
                let job_id_clone = job_id.clone();

                tokio::spawn(async move {
                    process_etc_job(jobs, running_count, job_id_clone, request).await;
                });
            }
        }

        job_id
    }

    /// Create a new ETC batch scrape job (multiple accounts)
    pub async fn create_etc_batch_job(&self, request: EtcBatchRequest, priority: JobPriority) -> String {
        let job_id = Uuid::new_v4().to_string();
        let total_count = request.accounts.len();

        // Initialize account results
        let account_results: Vec<AccountResult> = request
            .accounts
            .iter()
            .map(|a| {
                let name = if a.name.is_empty() {
                    a.user_id.clone()
                } else {
                    a.name.clone()
                };
                AccountResult::new(a.user_id.clone(), name)
            })
            .collect();

        // Create session folder with timestamp (JST)
        let session_folder = format!(
            "{}/{}",
            request.download_path,
            Self::format_jst_timestamp()
        );

        let job = Job {
            id: job_id.clone(),
            job_type: JobType::EtcScrapeBatch,
            priority: priority.clone(),
            status: if priority == JobPriority::Low {
                JobStatus::Queued
            } else {
                JobStatus::Pending
            },
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            vehicle_count: None,
            hono_response: None,
            etc_request: None,
            etc_result: None,
            batch_request: Some(request.clone()),
            batch_result: Some(EtcBatchResult {
                session_folder: session_folder.clone(),
                accounts: account_results,
                total_count,
                success_count: 0,
                fail_count: 0,
            }),
            current_account_index: Some(0),
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }

        match priority {
            JobPriority::Low => {
                // Add to queue, will be processed when idle
                let mut queue = self.queue.write().await;
                queue.push_back(QueuedJob {
                    id: job_id.clone(),
                    priority: priority.clone(),
                });
                info!(
                    "ETC batch job {} added to queue (low priority, {} accounts)",
                    job_id, total_count
                );
            }
            _ => {
                // Start processing immediately
                let jobs = self.jobs.clone();
                let running_count = self.running_count.clone();
                let job_id_clone = job_id.clone();

                tokio::spawn(async move {
                    process_etc_batch_job(jobs, running_count, job_id_clone, request, session_folder).await;
                });
            }
        }

        job_id
    }

    /// Process queued low-priority jobs when idle
    pub async fn process_idle_jobs(&self) {
        // Only process if idle
        if !self.is_idle().await {
            return;
        }

        // Get a job from the queue
        let queued_job = {
            let mut queue = self.queue.write().await;
            queue.pop_front()
        };

        if let Some(queued) = queued_job {
            // Get the job details
            let (job_type, etc_request, batch_request, session_folder) = {
                let jobs = self.jobs.read().await;
                if let Some(job) = jobs.get(&queued.id) {
                    let sf = job.batch_result.as_ref().map(|r| r.session_folder.clone());
                    (Some(job.job_type.clone()), job.etc_request.clone(), job.batch_request.clone(), sf)
                } else {
                    (None, None, None, None)
                }
            };

            // Update status to pending
            {
                let mut jobs = self.jobs.write().await;
                if let Some(job) = jobs.get_mut(&queued.id) {
                    job.status = JobStatus::Pending;
                }
            }

            match job_type {
                Some(JobType::EtcScrape) => {
                    if let Some(request) = etc_request {
                        info!("Processing queued ETC job {} (idle)", queued.id);
                        let jobs = self.jobs.clone();
                        let running_count = self.running_count.clone();
                        let job_id = queued.id;

                        tokio::spawn(async move {
                            process_etc_job(jobs, running_count, job_id, request).await;
                        });
                    }
                }
                Some(JobType::EtcScrapeBatch) => {
                    if let (Some(request), Some(sf)) = (batch_request, session_folder) {
                        info!("Processing queued ETC batch job {} (idle)", queued.id);
                        let jobs = self.jobs.clone();
                        let running_count = self.running_count.clone();
                        let job_id = queued.id;

                        tokio::spawn(async move {
                            process_etc_batch_job(jobs, running_count, job_id, request, sf).await;
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// Get queue length
    pub async fn queue_length(&self) -> usize {
        let queue = self.queue.read().await;
        queue.len()
    }

    /// Get a job by ID
    pub async fn get_job(&self, job_id: &str) -> Option<Job> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).cloned()
    }

    /// Get all jobs
    pub async fn get_all_jobs(&self) -> Vec<Job> {
        let jobs = self.jobs.read().await;
        jobs.values().cloned().collect()
    }

    /// Get jobs by type
    pub async fn get_jobs_by_type(&self, job_type: JobType) -> Vec<Job> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| j.job_type == job_type)
            .cloned()
            .collect()
    }
}

/// Process a vehicle job in the background using DtakologScraper
async fn process_vehicle_job(
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    config: Arc<Config>,
    running_count: Arc<RwLock<usize>>,
    job_id: String,
) {
    use scraper_service::{DtakologConfig, DtakologScraper};

    // Increment running count
    {
        let mut count = running_count.write().await;
        *count += 1;
    }

    // Update status to running
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.status = JobStatus::Running;
            job.started_at = Some(Utc::now());
            info!("Vehicle job {} status updated to running", job_id);
        }
    }

    // Create DtakologScraper config from app config
    let dtakolog_config = DtakologConfig {
        comp_id: config.comp_id.clone(),
        user_name: config.user_name.clone(),
        user_pass: config.user_pass.clone(),
        branch_id: "00000000".to_string(),
        filter_id: "0".to_string(),
        headless: config.browser_headless,
        debug: config.browser_debug,
        session_ttl_secs: config.session_ttl.as_secs(),
        // gRPC送信はmanager.rs側で行うので、scraper側には渡さない
        grpc_url: None,
        grpc_organization_id: None,
    };

    // Create and initialize scraper
    let mut scraper = DtakologScraper::new(dtakolog_config);

    let result = async {
        scraper.initialize().await?;
        scraper.scrape(None, false).await
    }
    .await;

    // Send to gRPC if enabled and data was retrieved
    let hono_response = match &result {
        Ok(scrape_result) if !config.rust_logi_url.is_empty() => {
            match send_to_rust_logi(&config, &scrape_result.raw_data).await {
                Ok(resp) => Some(resp),
                Err(e) => {
                    warn!("Failed to send to rust-logi: {}", e);
                    None
                }
            }
        }
        _ => None,
    };

    // Send DVR notifications to rust-logi if any
    if let Ok(ref scrape_result) = result {
        if !scrape_result.video_notifications.is_empty() && !config.rust_logi_url.is_empty() {
            if let Err(e) =
                send_dvr_notifications_to_rust_logi(&config, &scrape_result.video_notifications)
                    .await
            {
                warn!("Failed to send DVR notifications to rust-logi: {}", e);
            }
        }
    }

    // Update job with results
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.completed_at = Some(Utc::now());

            match result {
                Ok(scrape_result) => {
                    job.status = JobStatus::Completed;
                    job.vehicle_count = Some(scrape_result.vehicles.len() as i32);
                    job.hono_response = hono_response.clone();
                    info!(
                        "Vehicle job {} completed successfully with {} vehicles",
                        job_id,
                        scrape_result.vehicles.len()
                    );

                    if let Some(ref resp) = hono_response {
                        info!(
                            "rust-logi Response for job {} - Success: {}, Records: {}/{}",
                            job_id, resp.success, resp.records_added, resp.total_records
                        );
                    }
                }
                Err(e) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(e.to_string());
                    error!("Vehicle job {} failed: {}", job_id, e);
                }
            }
        }
    }

    // Close scraper
    if let Err(e) = scraper.close().await {
        warn!("Failed to close scraper: {}", e);
    }

    // Decrement running count
    {
        let mut count = running_count.write().await;
        *count = count.saturating_sub(1);
    }

    // Clean up old jobs after 10 minutes
    let jobs_cleanup = jobs.clone();
    let job_id_cleanup = job_id.clone();

    tokio::spawn(async move {
        sleep(Duration::from_secs(600)).await;
        let mut jobs_write = jobs_cleanup.write().await;
        jobs_write.remove(&job_id_cleanup);
        info!("Job {} cleaned up", job_id_cleanup);
    });
}

/// Get ID token from GCE metadata server for Cloud Run authentication
#[cfg(feature = "grpc")]
async fn get_gce_id_token(audience: &str) -> Result<String, String> {
    let metadata_url = format!(
        "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/identity?audience={}",
        audience
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&metadata_url)
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch ID token: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Metadata server returned status: {}", response.status()));
    }

    response
        .text()
        .await
        .map_err(|e| format!("Failed to read ID token: {}", e))
}

/// Send raw data to rust-logi via gRPC
#[cfg(feature = "grpc")]
async fn send_to_rust_logi(
    config: &Config,
    raw_data: &[serde_json::Value],
) -> Result<HonoApiResponse, String> {
    use crate::logi::dtakologs::dtakologs_service_client::DtakologsServiceClient;
    use crate::logi::dtakologs::{BulkCreateDtakologsRequest, Dtakolog};
    use tonic::metadata::MetadataValue;
    use tonic::Request;

    if config.rust_logi_url.is_empty() {
        return Err("RUST_LOGI_URL not configured".to_string());
    }

    info!(
        "send_to_rust_logi: Sending {} records to {}",
        raw_data.len(),
        config.rust_logi_url
    );

    // Get ID token for Cloud Run authentication (only for HTTPS URLs)
    let id_token = if config.rust_logi_url.starts_with("https://") {
        match get_gce_id_token(&config.rust_logi_url).await {
            Ok(token) => {
                info!("Got ID token for Cloud Run authentication");
                Some(token)
            }
            Err(e) => {
                warn!("Failed to get ID token (not running on GCE?): {}", e);
                None
            }
        }
    } else {
        None
    };

    // Create gRPC client with TLS for HTTPS URLs
    let channel = if config.rust_logi_url.starts_with("https://") {
        info!("Connecting with TLS (webpki-roots) to {}", config.rust_logi_url);
        tonic::transport::Channel::from_shared(config.rust_logi_url.clone())
            .map_err(|e| format!("Invalid URL: {}", e))?
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| format!("TLS config failed: {}", e))?
            .connect()
            .await
            .map_err(|e| format!("Connection failed: {:?}", e))?
    } else {
        info!("Connecting without TLS to {}", config.rust_logi_url);
        tonic::transport::Channel::from_shared(config.rust_logi_url.clone())
            .map_err(|e| format!("Invalid URL: {}", e))?
            .connect()
            .await
            .map_err(|e| format!("Connection failed: {:?}", e))?
    };

    let mut client = DtakologsServiceClient::new(channel);

    // Convert raw data to Dtakolog messages
    let dtakologs: Vec<Dtakolog> = raw_data
        .iter()
        .filter_map(|v| {
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
                data_date_time: convert_data_date_time(obj.get("DataDateTime").and_then(|v| v.as_str()).unwrap_or("")),
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
                speed: obj.get("Speed").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
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

    // Create request with headers
    let mut request = Request::new(BulkCreateDtakologsRequest { dtakologs });

    // Add ID token for Cloud Run authentication
    if let Some(ref token) = id_token {
        let auth_value = format!("Bearer {}", token);
        request.metadata_mut().insert(
            "authorization",
            MetadataValue::try_from(&auth_value)
                .map_err(|e| format!("Invalid authorization header: {}", e))?,
        );
    }

    // Add organization ID header
    if !config.rust_logi_organization_id.is_empty() {
        request.metadata_mut().insert(
            "x-organization-id",
            MetadataValue::try_from(&config.rust_logi_organization_id)
                .map_err(|e| format!("Invalid organization ID: {}", e))?,
        );
    }

    // Send request
    let response = client
        .bulk_create(request)
        .await
        .map_err(|e| format!("BulkCreate failed: {}", e))?;

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
#[cfg(feature = "grpc")]
fn convert_data_date_time(date_str: &str) -> String {
    use chrono::offset::FixedOffset;

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

/// Stub for non-grpc feature
#[cfg(not(feature = "grpc"))]
async fn send_to_rust_logi(
    _config: &Config,
    _raw_data: &[serde_json::Value],
) -> Result<HonoApiResponse, String> {
    warn!("grpc feature not enabled - data not sent to rust-logi");
    Err("grpc feature not enabled".to_string())
}

/// Send DVR video notifications to rust-logi via gRPC
#[cfg(feature = "grpc")]
async fn send_dvr_notifications_to_rust_logi(
    config: &Config,
    notifications: &[scraper_service::VideoNotificationResult],
) -> Result<(), String> {
    use crate::logi::dvr_notifications::dvr_notifications_service_client::DvrNotificationsServiceClient;
    use crate::logi::dvr_notifications::{BulkCreateDvrNotificationsRequest, DvrNotification};
    use tonic::metadata::MetadataValue;
    use tonic::Request;

    if config.rust_logi_url.is_empty() {
        return Err("RUST_LOGI_URL not configured".to_string());
    }

    if notifications.is_empty() {
        info!("No DVR notifications to send");
        return Ok(());
    }

    info!(
        "send_dvr_notifications: Sending {} notifications to {}",
        notifications.len(),
        config.rust_logi_url
    );

    // Get ID token for Cloud Run authentication (only for HTTPS URLs)
    let id_token = if config.rust_logi_url.starts_with("https://") {
        match get_gce_id_token(&config.rust_logi_url).await {
            Ok(token) => Some(token),
            Err(e) => {
                warn!("Failed to get ID token (not running on GCE?): {}", e);
                None
            }
        }
    } else {
        None
    };

    // Create gRPC client with TLS for HTTPS URLs
    let channel = if config.rust_logi_url.starts_with("https://") {
        tonic::transport::Channel::from_shared(config.rust_logi_url.clone())
            .map_err(|e| format!("Invalid URL: {}", e))?
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| format!("TLS config failed: {}", e))?
            .connect()
            .await
            .map_err(|e| format!("Connection failed: {:?}", e))?
    } else {
        tonic::transport::Channel::from_shared(config.rust_logi_url.clone())
            .map_err(|e| format!("Invalid URL: {}", e))?
            .connect()
            .await
            .map_err(|e| format!("Connection failed: {:?}", e))?
    };

    let mut client = DvrNotificationsServiceClient::new(channel);

    // Convert to proto messages
    let dvr_notifications: Vec<DvrNotification> = notifications
        .iter()
        .map(|n| DvrNotification {
            vehicle_cd: n.vehicle_cd,
            vehicle_name: n.vehicle_name.clone(),
            serial_no: n.serial_no.clone(),
            file_name: n.file_name.clone(),
            event_type: n.event_type.clone(),
            dvr_datetime: n.dvr_datetime.clone(),
            driver_name: n.driver_name.clone(),
            mp4_url: n.mp4_url.clone(),
        })
        .collect();

    // Create request with headers
    let mut request = Request::new(BulkCreateDvrNotificationsRequest {
        notifications: dvr_notifications,
    });

    // Add ID token for Cloud Run authentication
    if let Some(ref token) = id_token {
        let auth_value = format!("Bearer {}", token);
        request.metadata_mut().insert(
            "authorization",
            MetadataValue::try_from(&auth_value)
                .map_err(|e| format!("Invalid authorization header: {}", e))?,
        );
    }

    // Add organization ID header
    if !config.rust_logi_organization_id.is_empty() {
        request.metadata_mut().insert(
            "x-organization-id",
            MetadataValue::try_from(&config.rust_logi_organization_id)
                .map_err(|e| format!("Invalid organization ID: {}", e))?,
        );
    }

    // Send request
    let response = client
        .bulk_create(request)
        .await
        .map_err(|e| format!("DVR BulkCreate failed: {}", e))?;

    let resp = response.into_inner();
    info!(
        "DVR notifications sent: success={}, records_added={}, total_records={}, message={}",
        resp.success, resp.records_added, resp.total_records, resp.message
    );

    Ok(())
}

/// Stub for non-grpc feature
#[cfg(not(feature = "grpc"))]
async fn send_dvr_notifications_to_rust_logi(
    _config: &Config,
    _notifications: &[scraper_service::VideoNotificationResult],
) -> Result<(), String> {
    warn!("grpc feature not enabled - DVR notifications not sent");
    Err("grpc feature not enabled".to_string())
}

/// Process an ETC scrape job in the background
async fn process_etc_job(
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    running_count: Arc<RwLock<usize>>,
    job_id: String,
    request: EtcScrapeRequest,
) {
    // Increment running count
    {
        let mut count = running_count.write().await;
        *count += 1;
    }

    // Update status to running
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.status = JobStatus::Running;
            job.started_at = Some(Utc::now());
            info!("ETC job {} status updated to running", job_id);
        }
    }

    // Execute ETC scrape
    let result = execute_etc_scrape(&request).await;

    // Update job with results
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.completed_at = Some(Utc::now());

            match result {
                Ok(scrape_result) => {
                    job.status = JobStatus::Completed;
                    job.etc_result = Some(scrape_result.clone());
                    info!(
                        "ETC job {} completed successfully: {:?}",
                        job_id, scrape_result.csv_path
                    );
                }
                Err(e) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(e.to_string());
                    error!("ETC job {} failed: {}", job_id, e);
                }
            }
        }
    }

    // Decrement running count
    {
        let mut count = running_count.write().await;
        *count = count.saturating_sub(1);
    }

    // Clean up old jobs after 10 minutes
    let jobs_cleanup = jobs.clone();
    let job_id_cleanup = job_id.clone();

    tokio::spawn(async move {
        sleep(Duration::from_secs(600)).await;
        let mut jobs_write = jobs_cleanup.write().await;
        jobs_write.remove(&job_id_cleanup);
        info!("Job {} cleaned up", job_id_cleanup);
    });
}

/// Execute ETC scrape using scraper-service
async fn execute_etc_scrape(request: &EtcScrapeRequest) -> Result<EtcScrapeResult, String> {
    use scraper_service::{EtcScraper, ScraperConfig, Scraper};

    info!("Starting ETC scrape for user: {}", request.user_id);

    let config = ScraperConfig::new(&request.user_id, &request.password)
        .with_download_path(&request.download_path)
        .with_headless(request.headless)
        .with_timeout(default_download_timeout());

    let mut scraper = EtcScraper::new(config);

    let csv_path = scraper
        .execute()
        .await
        .map_err(|e| format!("ETC scrape failed: {}", e))?;

    let csv_size = std::fs::metadata(&csv_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);

    Ok(EtcScrapeResult {
        csv_path: csv_path.to_string_lossy().to_string(),
        csv_size,
    })
}

/// Process an ETC batch scrape job in the background
async fn process_etc_batch_job(
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    running_count: Arc<RwLock<usize>>,
    job_id: String,
    request: EtcBatchRequest,
    session_folder: String,
) {
    use scraper_service::{EtcScraper, ScraperConfig, ScraperError, Scraper};

    // Increment running count
    {
        let mut count = running_count.write().await;
        *count += 1;
    }

    // Update status to running
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.status = JobStatus::Running;
            job.started_at = Some(Utc::now());
            info!(
                "ETC batch job {} status updated to running ({} accounts)",
                job_id,
                request.accounts.len()
            );
        }
    }

    // Create session folder
    let session_path = PathBuf::from(&session_folder);
    if let Err(e) = std::fs::create_dir_all(&session_path) {
        error!("Failed to create session folder {}: {}", session_folder, e);
    }

    let mut success_count = 0usize;
    let mut fail_count = 0usize;

    // Process each account sequentially
    for (index, account) in request.accounts.iter().enumerate() {
        let user_id = &account.user_id;
        let name = if account.name.is_empty() {
            user_id.clone()
        } else {
            account.name.clone()
        };

        info!(
            "ETC batch job {}: Processing account {}/{} ({})",
            job_id,
            index + 1,
            request.accounts.len(),
            name
        );

        // Update current account status to running
        {
            let mut jobs_write = jobs.write().await;
            if let Some(job) = jobs_write.get_mut(&job_id) {
                job.current_account_index = Some(index);
                if let Some(ref mut batch_result) = job.batch_result {
                    if let Some(account_result) = batch_result.accounts.get_mut(index) {
                        account_result.set_running();
                    }
                }
            }
        }

        // Execute scrape for this account
        let config = ScraperConfig::new(user_id, &account.password)
            .with_download_path(&session_folder)
            .with_headless(request.headless)
            .with_timeout(default_download_timeout());

        let mut scraper = EtcScraper::new(config);
        let result = scraper.execute().await;

        // Update account result
        {
            let mut jobs_write = jobs.write().await;
            if let Some(job) = jobs_write.get_mut(&job_id) {
                if let Some(ref mut batch_result) = job.batch_result {
                    if let Some(account_result) = batch_result.accounts.get_mut(index) {
                        match result {
                            Ok(csv_path) => {
                                let csv_size = std::fs::metadata(&csv_path)
                                    .map(|m| m.len() as usize)
                                    .unwrap_or(0);
                                account_result.set_completed(
                                    csv_path.to_string_lossy().to_string(),
                                    csv_size,
                                );
                                success_count += 1;
                                info!(
                                    "ETC batch job {}: Account {} completed successfully",
                                    job_id, name
                                );
                            }
                            Err(ScraperError::NoUsageData(msg)) => {
                                // 明細なしは成功扱い（スキップ）
                                account_result.set_completed(
                                    format!("skipped: {}", msg),
                                    0,
                                );
                                success_count += 1;
                                info!(
                                    "ETC batch job {}: Account {} skipped (no usage data)",
                                    job_id, name
                                );
                            }
                            Err(e) => {
                                account_result.set_failed(e.to_string());
                                fail_count += 1;
                                error!(
                                    "ETC batch job {}: Account {} failed: {}",
                                    job_id, name, e
                                );
                            }
                        }
                    }
                    batch_result.success_count = success_count;
                    batch_result.fail_count = fail_count;
                }
            }
        }
    }

    // Finalize job
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.completed_at = Some(Utc::now());
            job.current_account_index = None;

            if fail_count > 0 {
                job.status = JobStatus::Failed;
                job.error = Some(format!(
                    "{} of {} accounts failed",
                    fail_count,
                    request.accounts.len()
                ));
            } else {
                job.status = JobStatus::Completed;
            }

            info!(
                "ETC batch job {} completed: {}/{} succeeded, {}/{} failed",
                job_id,
                success_count,
                request.accounts.len(),
                fail_count,
                request.accounts.len()
            );
        }
    }

    // Decrement running count
    {
        let mut count = running_count.write().await;
        *count = count.saturating_sub(1);
    }

    // Clean up old session folders (keep latest 10)
    cleanup_old_session_folders(&request.download_path, 10);

    // Clean up old jobs after 10 minutes
    let jobs_cleanup = jobs.clone();
    let job_id_cleanup = job_id.clone();

    tokio::spawn(async move {
        sleep(Duration::from_secs(600)).await;
        let mut jobs_write = jobs_cleanup.write().await;
        jobs_write.remove(&job_id_cleanup);
        info!("Job {} cleaned up", job_id_cleanup);
    });
}

/// Clean up old session folders, keeping only the latest N folders
fn cleanup_old_session_folders(base_path: &str, keep_count: usize) {
    use std::fs;

    let base_dir = PathBuf::from(base_path);
    if !base_dir.exists() {
        return;
    }

    // Get all subdirectories with timestamp format (YYYYMMDD_HHMMSS)
    let mut folders: Vec<(PathBuf, String)> = match fs::read_dir(&base_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Check if name matches timestamp format (14 chars: YYYYMMDD_HHMMSS)
                if name.len() == 15 && name.chars().nth(8) == Some('_') {
                    Some((e.path(), name))
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            error!("Failed to read directory {}: {}", base_path, e);
            return;
        }
    };

    // Sort by name (timestamp) in descending order (newest first)
    folders.sort_by(|a, b| b.1.cmp(&a.1));

    // Remove folders beyond keep_count
    for (path, name) in folders.into_iter().skip(keep_count) {
        info!("Removing old session folder: {}", path.display());
        if let Err(e) = fs::remove_dir_all(&path) {
            error!("Failed to remove folder {}: {}", name, e);
        }
    }
}

/// Start idle job processor task
pub fn start_idle_processor(job_manager: Arc<JobManager>) {
    tokio::spawn(async move {
        loop {
            // Check every 30 seconds
            sleep(Duration::from_secs(30)).await;

            let queue_len = job_manager.queue_length().await;
            if queue_len > 0 && job_manager.is_idle().await {
                info!("Idle detected, processing queued jobs ({} in queue)", queue_len);
                job_manager.process_idle_jobs().await;
            }
        }
    });
}
