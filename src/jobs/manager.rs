use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use tracing::{error, info};
use uuid::Uuid;

use crate::browser::{HonoApiResponse, Renderer};

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
    renderer: Arc<Renderer>,
    running_count: Arc<RwLock<usize>>,
}

impl JobManager {
    /// Create a new job manager
    pub fn new(renderer: Arc<Renderer>) -> Self {
        JobManager {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            queue: Arc::new(RwLock::new(VecDeque::new())),
            renderer,
            running_count: Arc::new(RwLock::new(0)),
        }
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

        // Start processing in background
        let jobs = self.jobs.clone();
        let renderer = self.renderer.clone();
        let running_count = self.running_count.clone();
        let job_id_clone = job_id.clone();

        tokio::spawn(async move {
            process_vehicle_job(jobs, renderer, running_count, job_id_clone).await;
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

        // Create session folder with timestamp
        let session_folder = format!(
            "{}/{}",
            request.download_path,
            Utc::now().format("%Y%m%d_%H%M%S")
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

/// Process a vehicle job in the background
async fn process_vehicle_job(
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    renderer: Arc<Renderer>,
    running_count: Arc<RwLock<usize>>,
    job_id: String,
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
            info!("Vehicle job {} status updated to running", job_id);
        }
    }

    // Call the renderer with fixed parameters
    let result = renderer
        .get_vehicle_data(
            "",         // Session ID
            "00000000", // Branch ID
            "0",        // Filter ID
            false,      // Force login
        )
        .await;

    // Update job with results
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.completed_at = Some(Utc::now());

            match result {
                Ok((vehicles, _session_id, hono_response)) => {
                    job.status = JobStatus::Completed;
                    job.vehicle_count = Some(vehicles.len() as i32);
                    job.hono_response = hono_response.clone();
                    info!(
                        "Vehicle job {} completed successfully with {} vehicles",
                        job_id,
                        vehicles.len()
                    );

                    if let Some(ref resp) = hono_response {
                        info!(
                            "Hono API Response for job {} - Success: {}, Records: {}/{}",
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
    use scraper_service::{EtcScraper, ScraperConfig, Scraper};

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
