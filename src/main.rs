mod browser;
mod config;
mod jobs;
mod logging;
mod server;
mod storage;

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use tokio::signal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use browser::Renderer;
use config::{Config, LogFormat, LogRotation};
use jobs::{start_idle_processor, JobManager};
use server::{start_http_server, AppState};
use storage::Storage;

#[cfg(feature = "grpc")]
use server::start_grpc_server;

/// Browser Render Service
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// gRPC server port (overrides env)
    #[arg(long)]
    grpc_port: Option<String>,

    /// HTTP server port (overrides env)
    #[arg(long)]
    http_port: Option<String>,

    /// SQLite database path (overrides env)
    #[arg(long)]
    db_path: Option<String>,

    /// Run browser in headless mode
    #[arg(long, default_value = "true")]
    headless: bool,

    /// Enable debug mode
    #[arg(long, default_value = "false")]
    debug: bool,

    /// Server type: grpc, http, or both
    #[arg(long, default_value = "http")]
    server: String,

    /// Log output format: text or json
    #[arg(long, default_value = "text")]
    log_format: String,

    /// Log output file name (enables file output when specified)
    #[arg(long)]
    log_file: Option<String>,

    /// Log directory for file output
    #[arg(long, default_value = "./logs")]
    log_dir: String,

    /// Log file rotation: daily, hourly, or never
    #[arg(long, default_value = "daily")]
    log_rotation: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Load configuration
    let mut cfg = Config::load();

    // Override config with command line flags
    if let Some(ref port) = args.grpc_port {
        cfg.grpc_port = port.clone();
    }
    if let Some(ref port) = args.http_port {
        cfg.http_port = port.clone();
    }
    if let Some(ref path) = args.db_path {
        cfg.sqlite_path = path.clone();
    }
    cfg.browser_headless = args.headless;
    cfg.browser_debug = args.debug;

    // Apply logging-specific overrides from CLI
    cfg.log_format = LogFormat::from_str(&args.log_format);
    if let Some(ref file) = args.log_file {
        cfg.log_file = Some(file.clone());
    }
    cfg.log_dir = args.log_dir.clone();
    cfg.log_rotation = LogRotation::from_str(&args.log_rotation);

    // Initialize logging (guard must be held until shutdown)
    let _logging_guard = logging::init_logging(&cfg);

    info!("Starting Browser Render Rust Server...");
    info!(
        "Configuration loaded: gRPC={}, HTTP={}",
        cfg.grpc_port, cfg.http_port
    );

    // Initialize storage
    let storage = Arc::new(Storage::new(&cfg.sqlite_path).await?);
    info!("Storage initialized successfully");

    // Initialize browser renderer
    let config = Arc::new(cfg.clone());
    let renderer = Arc::new(Renderer::new(config.clone(), storage.clone()).await?);
    info!("Browser renderer initialized successfully");

    // Create job manager
    let job_manager = Arc::new(JobManager::new(renderer.clone()));

    // Start idle job processor (processes low-priority jobs when system is idle)
    start_idle_processor(job_manager.clone());
    info!("Idle job processor started");

    // Create shutdown channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Start cleanup task
    let storage_cleanup = storage.clone();
    let mut shutdown_rx_cleanup = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = storage_cleanup.cleanup_expired().await {
                        error!("Error cleaning up expired data: {}", e);
                    }
                }
                _ = shutdown_rx_cleanup.recv() => {
                    info!("Cleanup task shutting down");
                    break;
                }
            }
        }
    });

    // Start servers based on server type
    let server_type = args.server.as_str();

    match server_type {
        #[cfg(feature = "grpc")]
        "grpc" => {
            let grpc_addr = format!("0.0.0.0:{}", cfg.grpc_port);
            info!("Starting gRPC server on {}", grpc_addr);

            let config_grpc = config.clone();
            let storage_grpc = storage.clone();
            let renderer_grpc = renderer.clone();

            tokio::spawn(async move {
                if let Err(e) = start_grpc_server(config_grpc, storage_grpc, renderer_grpc, &grpc_addr).await {
                    error!("gRPC server error: {}", e);
                }
            });
        }
        #[cfg(not(feature = "grpc"))]
        "grpc" => {
            error!("gRPC support not enabled. Build with --features grpc");
            return Err(anyhow::anyhow!("gRPC support not enabled"));
        }
        "http" => {
            let http_addr = format!("0.0.0.0:{}", cfg.http_port);
            info!("Starting HTTP server on {}", http_addr);

            let state = Arc::new(AppState {
                config: config.clone(),
                storage: storage.clone(),
                renderer: renderer.clone(),
                job_manager: job_manager.clone(),
                start_time: Instant::now(),
            });

            tokio::spawn(async move {
                if let Err(e) = start_http_server(state, &http_addr).await {
                    error!("HTTP server error: {}", e);
                }
            });
        }
        "both" => {
            // Start gRPC server if enabled
            #[cfg(feature = "grpc")]
            {
                let grpc_addr = format!("0.0.0.0:{}", cfg.grpc_port);
                info!("Starting gRPC server on {}", grpc_addr);

                let config_grpc = config.clone();
                let storage_grpc = storage.clone();
                let renderer_grpc = renderer.clone();

                tokio::spawn(async move {
                    if let Err(e) = start_grpc_server(config_grpc, storage_grpc, renderer_grpc, &grpc_addr).await {
                        error!("gRPC server error: {}", e);
                    }
                });
            }

            #[cfg(not(feature = "grpc"))]
            {
                warn!("gRPC support not enabled. Only starting HTTP server.");
            }

            // Start HTTP server
            let http_addr = format!("0.0.0.0:{}", cfg.http_port);
            info!("Starting HTTP server on {}", http_addr);

            let state = Arc::new(AppState {
                config: config.clone(),
                storage: storage.clone(),
                renderer: renderer.clone(),
                job_manager: job_manager.clone(),
                start_time: Instant::now(),
            });

            tokio::spawn(async move {
                if let Err(e) = start_http_server(state, &http_addr).await {
                    error!("HTTP server error: {}", e);
                }
            });
        }
        _ => {
            error!("Invalid server type: {} (must be grpc, http, or both)", server_type);
            return Err(anyhow::anyhow!("Invalid server type"));
        }
    }

    // Print startup information
    print_startup_info(&cfg, server_type);

    // Wait for shutdown signal
    info!("Press Ctrl+C to stop the server");

    signal::ctrl_c().await?;
    info!("Received shutdown signal. Shutting down gracefully...");

    // Send shutdown signal (receiver may already be dropped, which is fine)
    if shutdown_tx.send(()).is_err() {
        debug!("Shutdown signal receiver already dropped");
    }

    // Give some time for graceful shutdown
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Close storage
    storage.close().await;

    info!("Shutdown complete");
    Ok(())
}

fn print_startup_info(cfg: &Config, server_type: &str) {
    println!();
    println!("{}", "=".repeat(50));
    println!("Browser Render Rust Server Started");
    println!("{}", "=".repeat(50));

    match server_type {
        "grpc" => {
            println!("gRPC Server:  localhost:{}", cfg.grpc_port);
        }
        "http" => {
            println!("HTTP Server:  http://localhost:{}", cfg.http_port);
            println!("Health Check: http://localhost:{}/health", cfg.http_port);
        }
        "both" => {
            #[cfg(feature = "grpc")]
            println!("gRPC Server:  localhost:{}", cfg.grpc_port);
            println!("HTTP Server:  http://localhost:{}", cfg.http_port);
            println!("Health Check: http://localhost:{}/health", cfg.http_port);
        }
        _ => {}
    }

    println!();
    println!("Configuration:");
    println!(
        "  Browser Mode: {}",
        if cfg.browser_headless {
            "Headless"
        } else {
            "GUI"
        }
    );
    println!("  Debug Mode:   {}", cfg.browser_debug);
    println!("  Database:     {}", cfg.sqlite_path);
    println!();
    println!("Press Ctrl+C to stop the server");
    println!("{}", "=".repeat(50));
    println!();
}
