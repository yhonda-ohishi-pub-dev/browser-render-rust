use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use tracing::{error, info};
use uuid::Uuid;

use crate::browser::{HonoApiResponse, Renderer};

/// Job status enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed => write!(f, "completed"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Job data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hono_response: Option<HonoApiResponse>,
}

/// Job manager for handling background jobs
pub struct JobManager {
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    renderer: Arc<Renderer>,
}

impl JobManager {
    /// Create a new job manager
    pub fn new(renderer: Arc<Renderer>) -> Self {
        JobManager {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            renderer,
        }
    }

    /// Create a new job and start processing in background
    pub async fn create_job(&self) -> String {
        let job_id = Uuid::new_v4().to_string();

        let job = Job {
            id: job_id.clone(),
            status: JobStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
            error: None,
            vehicle_count: None,
            hono_response: None,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }

        // Start processing in background
        let jobs = self.jobs.clone();
        let renderer = self.renderer.clone();
        let job_id_clone = job_id.clone();

        tokio::spawn(async move {
            process_job(jobs, renderer, job_id_clone).await;
        });

        job_id
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
}

/// Process a job in the background
async fn process_job(
    jobs: Arc<RwLock<HashMap<String, Job>>>,
    renderer: Arc<Renderer>,
    job_id: String,
) {
    // Update status to running
    {
        let mut jobs_write = jobs.write().await;
        if let Some(job) = jobs_write.get_mut(&job_id) {
            job.status = JobStatus::Running;
            info!("Job {} status updated to running", job_id);
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
                        "Job {} completed successfully with {} vehicles",
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
                    error!("Job {} failed: {}", job_id, e);
                }
            }
        }
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
