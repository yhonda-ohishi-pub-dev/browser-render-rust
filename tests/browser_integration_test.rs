//! Integration tests for browser automation with DtakologScraper
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

use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize environment from .env file (only once)
fn init_env() {
    INIT.call_once(|| {
        let _ = dotenvy::dotenv();
    });
}

use scraper_service::{DtakologConfig, DtakologScraper};

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
async fn test_vehicle_data_extraction() {
    // Skip if credentials not set
    if !credentials_available() {
        eprintln!("Skipping test: Credentials not set (USER_NAME, COMP_ID, USER_PASS)");
        return;
    }

    println!("=== Starting integration test ===");

    // Create DtakologScraper config from environment
    let config = DtakologConfig {
        comp_id: std::env::var("COMP_ID").unwrap(),
        user_name: std::env::var("USER_NAME").unwrap(),
        user_pass: std::env::var("USER_PASS").unwrap(),
        branch_id: "00000000".to_string(),
        filter_id: "0".to_string(),
        headless: true,
        debug: false,
        session_ttl_secs: 1800,
        grpc_url: None,              // No gRPC for this test
        grpc_organization_id: None,
    };

    let mut scraper = DtakologScraper::new(config);

    // Initialize scraper
    println!("Initializing scraper...");
    let init_result = scraper.initialize().await;
    assert!(init_result.is_ok(), "Failed to initialize scraper: {:?}", init_result.err());

    // Execute scrape
    println!("Executing scrape...");
    let result = scraper.scrape(None, false).await;

    match &result {
        Ok(scrape_result) => {
            println!("Success! Extracted {} vehicles", scrape_result.vehicles.len());
            println!("Raw data count: {}", scrape_result.raw_data.len());
        }
        Err(e) => {
            eprintln!("Error: {:?}", e);
        }
    }

    assert!(result.is_ok(), "Vehicle data extraction failed: {:?}", result.err());

    let scrape_result = result.unwrap();

    // Verify we got some vehicles
    assert!(!scrape_result.vehicles.is_empty(), "No vehicles extracted");
    println!("Extracted {} vehicles", scrape_result.vehicles.len());

    // Print first few vehicles for debugging
    for (i, vehicle) in scrape_result.vehicles.iter().take(3).enumerate() {
        println!(
            "Vehicle {}: {} - {}",
            i + 1,
            vehicle.vehicle_cd,
            vehicle.vehicle_name
        );
    }

    // Verify raw data structure
    assert!(!scrape_result.raw_data.is_empty(), "No raw data");
    for (i, item) in scrape_result.raw_data.iter().take(3).enumerate() {
        assert!(item.is_object(), "Raw data item {} is not an object", i);
        let obj = item.as_object().unwrap();
        assert!(obj.contains_key("VehicleCD"), "Item {} missing VehicleCD", i);
        assert!(obj.contains_key("VehicleName"), "Item {} missing VehicleName", i);
    }

    println!("=== All assertions passed ===");

    // Cleanup
    let _ = scraper.close().await;
}

#[tokio::test]
#[ignore]
async fn test_session_reuse() {
    if !credentials_available() {
        eprintln!("Skipping test: Credentials not set");
        return;
    }

    println!("=== Starting session reuse test ===");

    let config = DtakologConfig {
        comp_id: std::env::var("COMP_ID").unwrap(),
        user_name: std::env::var("USER_NAME").unwrap(),
        user_pass: std::env::var("USER_PASS").unwrap(),
        branch_id: "00000000".to_string(),
        filter_id: "0".to_string(),
        headless: true,
        debug: false,
        session_ttl_secs: 1800,
        grpc_url: None,
        grpc_organization_id: None,
    };

    let mut scraper = DtakologScraper::new(config);

    // First call - should create session
    scraper.initialize().await.expect("Failed to initialize");
    let result1 = scraper.scrape(None, false).await;
    assert!(result1.is_ok(), "First scrape failed: {:?}", result1.err());

    let vehicles1 = result1.unwrap().vehicles.len();
    println!("First scrape: {} vehicles", vehicles1);

    // Second call - should reuse session
    let result2 = scraper.scrape(None, false).await;
    assert!(result2.is_ok(), "Second scrape failed: {:?}", result2.err());

    let vehicles2 = result2.unwrap().vehicles.len();
    println!("Second scrape: {} vehicles", vehicles2);

    println!("=== Session reuse test passed ===");

    let _ = scraper.close().await;
}

#[tokio::test]
async fn test_config_creation() {
    // Test that DtakologConfig can be created
    let config = DtakologConfig {
        comp_id: "test_comp".to_string(),
        user_name: "test_user".to_string(),
        user_pass: "test_pass".to_string(),
        branch_id: "00000000".to_string(),
        filter_id: "0".to_string(),
        headless: true,
        debug: false,
        session_ttl_secs: 1800,
        grpc_url: Some("http://localhost:50051".to_string()),
        grpc_organization_id: Some("test-org-id".to_string()),
    };

    assert_eq!(config.comp_id, "test_comp");
    assert_eq!(config.user_name, "test_user");
    assert!(config.headless);
    assert!(config.grpc_url.is_some());

    println!("Config creation test passed");
}
