use std::collections::{HashMap, VecDeque};
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
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobType::Vehicle => write!(f, "vehicle"),
            JobType::EtcScrape => write!(f, "etc_scrape"),
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
    "./downloads".to_string()
}

fn default_headless() -> bool {
    true
}

/// ETC scrape result data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcScrapeResult {
    pub csv_path: String,
    pub csv_size: usize,
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
            let etc_request = {
                let jobs = self.jobs.read().await;
                jobs.get(&queued.id).and_then(|j| j.etc_request.clone())
            };

            if let Some(request) = etc_request {
                info!("Processing queued ETC job {} (idle)", queued.id);

                // Update status to pending
                {
                    let mut jobs = self.jobs.write().await;
                    if let Some(job) = jobs.get_mut(&queued.id) {
                        job.status = JobStatus::Pending;
                    }
                }

                let jobs = self.jobs.clone();
                let running_count = self.running_count.clone();
                let job_id = queued.id;

                tokio::spawn(async move {
                    process_etc_job(jobs, running_count, job_id, request).await;
                });
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
        .with_headless(request.headless);

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
