//! Integration tests for browser automation with mock Hono API server
//!
//! Run with: cargo test --test browser_integration_test -- --ignored --nocapture --test-threads=1
//!
//! Note: These tests must run sequentially (--test-threads=1) because they use
//! browser automation which can conflict when multiple instances run simultaneously.
//!
//! Requires environment variables (or .env file):
//! - USER_NAME: Login username
//! - COMP_ID: Company ID
//! - USER_PASS: Password

use std::sync::Arc;
use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize environment from .env file (only once)
fn init_env() {
    INIT.call_once(|| {
        let _ = dotenvy::dotenv();
    });
}

use axum::{extract::State, routing::post, Json, Router};
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::sync::{oneshot, RwLock};

use browser_render::browser::Renderer;
use browser_render::config::Config;
use browser_render::storage::Storage;

// =============================================================================
// Mock Hono Server
// =============================================================================

/// Mock server that captures POST requests for verification
pub struct MockHonoServer {
    pub url: String,
    pub received_data: Arc<RwLock<Vec<Value>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
struct MockServerState {
    received_data: Arc<RwLock<Vec<Value>>>,
}

impl MockHonoServer {
    /// Start mock server on a random available port
    pub async fn start() -> Self {
        let received_data = Arc::new(RwLock::new(Vec::new()));
        let state = MockServerState {
            received_data: received_data.clone(),
        };

        let app = Router::new()
            .route("/api/dtakologs", post(handle_post))
            .with_state(state);

        // Bind to port 0 for automatic port assignment
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        // Give server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        MockHonoServer {
            url,
            received_data,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Get all data received by the mock server
    pub async fn get_received_data(&self) -> Vec<Value> {
        self.received_data.read().await.clone()
    }

    /// Stop the mock server
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Handler for POST /api/dtakologs
async fn handle_post(
    State(state): State<MockServerState>,
    Json(body): Json<Vec<Value>>,
) -> Json<Value> {
    println!("Mock server received {} records", body.len());

    let mut data = state.received_data.write().await;
    data.extend(body);

    Json(serde_json::json!({
        "success": true,
        "message": "Data received by mock server"
    }))
}

// =============================================================================
// Test Context
// =============================================================================

/// Test context with mock server and configured renderer
pub struct TestContext {
    pub config: Arc<Config>,
    pub storage: Arc<Storage>,
    pub renderer: Renderer,
    pub mock_server: MockHonoServer,
    _db_file: NamedTempFile,
}

impl TestContext {
    /// Create a new test context with mock server and temporary database
    pub async fn new() -> Self {
        // Create temporary database file
        let db_file = NamedTempFile::new().expect("Failed to create temp file");
        let db_path = db_file.path().to_string_lossy().to_string();

        // Start mock server
        let mock_server = MockHonoServer::start().await;
        println!("Mock server started at: {}", mock_server.url);

        // Create config with mock server URL
        let mut config = Config::load();
        config.sqlite_path = db_path;
        config.hono_api_url = format!("{}/api/dtakologs", mock_server.url);
        config.browser_headless = true; // Force headless for tests

        let config = Arc::new(config);

        // Initialize storage
        let storage = Arc::new(
            Storage::new(&config.sqlite_path)
                .await
                .expect("Failed to initialize storage"),
        );

        // Initialize renderer
        let renderer = Renderer::new(config.clone(), storage.clone())
            .await
            .expect("Failed to initialize renderer");

        TestContext {
            config,
            storage,
            renderer,
            mock_server,
            _db_file: db_file,
        }
    }

    /// Cleanup test resources
    pub async fn cleanup(self) {
        self.storage.close().await;
        self.mock_server.stop().await;
    }
}

// =============================================================================
// Integration Tests
// =============================================================================

/// Check if required credentials are set
fn credentials_available() -> bool {
    init_env();
    std::env::var("USER_NAME").is_ok()
        && std::env::var("COMP_ID").is_ok()
        && std::env::var("USER_PASS").is_ok()
}

#[tokio::test]
#[ignore] // Run with: cargo test --test browser_integration_test -- --ignored
async fn test_vehicle_data_extraction_and_post() {
    // Skip if credentials not set
    if !credentials_available() {
        eprintln!("Skipping test: Credentials not set (USER_NAME, COMP_ID, USER_PASS)");
        return;
    }

    println!("=== Starting integration test ===");

    // Setup
    let ctx = TestContext::new().await;
    println!("Test context created");

    // Execute - perform login and data extraction
    println!("Calling get_vehicle_data...");
    let result = ctx
        .renderer
        .get_vehicle_data(
            "",         // session_id
            "00000000", // branch_id
            "0",        // filter_id
            true,       // force_login
        )
        .await;

    // Verify extraction succeeded
    match &result {
        Ok((vehicles, session_id, hono_response)) => {
            println!("Success! Extracted {} vehicles", vehicles.len());
            println!("Session ID: {}", session_id);
            println!("Hono response: {:?}", hono_response);
        }
        Err(e) => {
            eprintln!("Error: {:?}", e);
        }
    }

    assert!(
        result.is_ok(),
        "Vehicle data extraction failed: {:?}",
        result.err()
    );

    let (vehicles, session_id, hono_response) = result.unwrap();

    // Verify we got some vehicles
    assert!(!vehicles.is_empty(), "No vehicles extracted");
    println!("Extracted {} vehicles", vehicles.len());

    // Print first few vehicles for debugging
    for (i, vehicle) in vehicles.iter().take(3).enumerate() {
        println!(
            "Vehicle {}: {} - {}",
            i + 1,
            vehicle.vehicle_cd,
            vehicle.vehicle_name
        );
    }

    // Verify session was created
    assert!(!session_id.is_empty(), "Session ID was not created");

    // Verify Hono API response (from our mock)
    assert!(hono_response.is_some(), "No Hono API response received");
    let response = hono_response.unwrap();
    assert!(response.success, "Hono API response was not successful");

    // Verify mock server received the data
    let received = ctx.mock_server.get_received_data().await;
    assert!(!received.is_empty(), "Mock server received no data");
    println!("Mock server received {} records", received.len());

    // Verify data structure
    for (i, item) in received.iter().enumerate() {
        assert!(
            item.is_object(),
            "Received item {} is not an object",
            i
        );
        let obj = item.as_object().unwrap();

        // Check required fields exist (matching actual API response structure)
        assert!(
            obj.contains_key("VehicleCD"),
            "Item {} missing VehicleCD field",
            i
        );
        assert!(
            obj.contains_key("VehicleName"),
            "Item {} missing VehicleName field",
            i
        );
        // Note: The raw API data has "State", "State1", "State2", "State3" fields,
        // not a single "Status" field. We check for the actual structure.
        assert!(
            obj.contains_key("State") || obj.contains_key("AllState"),
            "Item {} missing State/AllState field",
            i
        );
    }

    println!("=== All assertions passed ===");

    // Cleanup
    ctx.cleanup().await;
}

#[tokio::test]
#[ignore]
async fn test_session_persistence() {
    if !credentials_available() {
        eprintln!("Skipping test: Credentials not set");
        return;
    }

    println!("=== Starting session persistence test ===");

    let ctx = TestContext::new().await;

    // First call - should create session
    let result = ctx
        .renderer
        .get_vehicle_data("", "00000000", "0", true)
        .await;

    assert!(result.is_ok(), "First call failed: {:?}", result.err());
    let (_, session_id, _) = result.unwrap();
    assert!(!session_id.is_empty(), "Session ID was not created");
    println!("Session created: {}", session_id);

    // Verify session stored in database
    let session = ctx.storage.get_session(&session_id).await;
    assert!(session.is_ok(), "Failed to query session");
    assert!(session.unwrap().is_some(), "Session not persisted to database");

    println!("=== Session persistence test passed ===");

    ctx.cleanup().await;
}

#[tokio::test]
async fn test_mock_server_standalone() {
    // Test that mock server works correctly
    let server = MockHonoServer::start().await;
    println!("Mock server URL: {}", server.url);

    // Send test data
    let client = reqwest::Client::new();
    let test_data = vec![serde_json::json!({
        "VehicleCD": "TEST001",
        "VehicleName": "Test Vehicle",
        "Status": "Active"
    })];

    let response = client
        .post(format!("{}/api/dtakologs", server.url))
        .json(&test_data)
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success());

    // Verify data was received
    let received = server.get_received_data().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["VehicleCD"], "TEST001");

    println!("Mock server standalone test passed");

    server.stop().await;
}
