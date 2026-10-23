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

    // 2. Initialize structured tracing subscriber
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "text".to_string());
    let default_filter = match config.environment {
        ai_backend::config::Environment::Production => "info,ai_backend=info",
        _ => "info,ai_backend=debug",
    };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    if log_format.eq_ignore_ascii_case("json") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .json()
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init();
    }

    let bind_addr = format!("{}:{}", config.host, config.port);
    let app_state = AppState::new(Arc::new(config.clone()));

    // 3. Start autonomous retention and retry maintenance worker
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let maintenance_handle =
        ai_backend::start_retention_maintenance(Arc::new(app_state.clone()), shutdown_rx);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = config.environment.as_str(),
        deployment_mode = config.deployment_mode.as_str(),
        max_body_size_bytes = config.max_body_size_bytes,
        "ai-backend starting"
    );

    // 4. Build the Axum application
    let app = ai_backend::create_app(app_state);

    // 5. Bind TCP listener asynchronously
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => {
            info!(
                addr = %bind_addr,
                health_probe = %format!("http://{bind_addr}/api/v1/health"),
                readiness_probe = %format!("http://{bind_addr}/api/v1/ready"),
                "Server bound and listening"
            );
            l
        }
        Err(err) => {
            error!(addr = %bind_addr, error = %err, "Failed to bind server socket");
            eprintln!("[FATAL] Failed to bind server to {bind_addr}: {err}");
            std::process::exit(1);
        }
    };

    // 5. Setup graceful shutdown signal handler
    let shutdown_signal = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C signal handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM signal handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }
        info!("Shutdown signal received, initiating graceful server termination");
    };

    // 6. Run the Axum server with graceful shutdown
    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
    {
        error!(error = %err, "Server encountered an unhandled fatal error");
        eprintln!("[FATAL] Server encountered an unhandled error: {err}");
        std::process::exit(1);
    }

    // 7. Signal autonomous maintenance task to complete cleanly
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), maintenance_handle).await;
    info!("ai-backend stopped gracefully");

    Ok(())
}
