//! Application entry point for ai-backend.
//!
//! Loads typed configuration from environment, validates startup invariants,
//! initializes shared application state, and starts the Axum HTTP server using Tokio.

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load and validate configuration before starting
    let config = match AppConfig::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("[FATAL] Configuration initialization failed: {err}");
            std::process::exit(1);
        }
    };

    let bind_addr = format!("{}:{}", config.host, config.port);
    let app_state = AppState::new(Arc::new(config.clone()));

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = config.environment.as_str(),
        deployment_mode = config.deployment_mode.as_str(),
        max_body_size_bytes = config.max_body_size_bytes,
        "ai-backend starting"
    );
    println!(
        "[INFO] ai-backend starting (version: {}, env: {}, mode: {})",
        env!("CARGO_PKG_VERSION"),
        config.environment.as_str(),
        config.deployment_mode.as_str()
    );

    // 2. Build the Axum application
    let app = ai_backend::create_app(app_state);

    // 3. Bind TCP listener asynchronously
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => {
            info!(addr = %bind_addr, "Server bound successfully");
            println!("[INFO] Server listening on http://{bind_addr}");
            println!("[INFO] Health probe available at http://{bind_addr}/api/v1/health");
            println!("[INFO] Readiness probe available at http://{bind_addr}/api/v1/ready");
            l
        }
        Err(err) => {
            error!(addr = %bind_addr, error = %err, "Failed to bind server socket");
            eprintln!("[FATAL] Failed to bind server to {bind_addr}: {err}");
            std::process::exit(1);
        }
    };

    // 4. Run the Axum server
    if let Err(err) = axum::serve(listener, app).await {
        error!(error = %err, "Server encountered an unhandled fatal error");
        eprintln!("[FATAL] Server encountered an unhandled error: {err}");
        std::process::exit(1);
    }

    Ok(())
}
