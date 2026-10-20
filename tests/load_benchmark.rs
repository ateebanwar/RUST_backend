//! Controlled Local Load and Performance Benchmark Suite.
//!
//! Evaluates local backend overhead, routing latency, and streaming performance
//! across 4 concurrency tiers (1, 5, 10, 20) with bounded requests.
//!
//! Marked `#[ignore]` so standard `cargo test` runs the authoritative 138-test
//! unit and regression suite without long-running benchmark overhead.
//! Execute on demand via:
//! `cargo test --test load_benchmark -- --ignored --nocapture`

use ai_backend::AppState;
use ai_backend::config::AppConfig;
use ai_backend::create_app;
use ai_backend::models::ModelMetadata;
use ai_backend::providers::{OpenAiCompatibleAdapter, ProviderRegistry};
use ai_backend::registry::ModelRegistry;
use ai_backend::router::ModelRouter;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// Spawns a local mock OpenAI-compatible upstream server to isolate backend overhead
/// from external network and provider rate limits.
async fn spawn_local_mock_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = match socket.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };

                let req_str = String::from_utf8_lossy(&buf[..n]);
                let is_stream =
                    req_str.contains("\"stream\":true") || req_str.contains("\"stream\": true");

                if is_stream {
                    let sse_body = "data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"Rust \"}}]}\n\n\
                                    data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"is \"}}]}\n\n\
                                    data: {\"id\":\"chatcmpl_mock_stream\",\"choices\":[{\"delta\":{\"content\":\"blazing fast.\"}}]}\n\n\
                                    data: [DONE]\n\n";

                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                        sse_body.len(),
                        sse_body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                } else {
                    let json_body = json!({
                        "id": "chatcmpl_mock_loadtest",
                        "model": "openai/gpt-oss-120b",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "Benchmark response from simulated local provider."
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 18,
                            "completion_tokens": 8,
                            "total_tokens": 26
                        }
                    });
                    let body_bytes = serde_json::to_vec(&json_body).unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body_bytes.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.write_all(&body_bytes).await;
                }
            });
        }
    });

    (format!("http://{}", addr), handle)
}

/// Helper to setup a test backend pointing to the local mock upstream.
async fn setup_benchmark_backend(upstream_url: &str) -> (String, tokio::task::JoinHandle<()>) {
    let mut config = AppConfig::default_local();
    config.api_key = Some("test_load_api_key_secure_99".to_string());
    config.groq_api_key = Some("mock_groq_key_val".to_string());
    config.groq_base_url = Some(upstream_url.to_string());
    let config = Arc::new(config);

    let registry = Arc::new(ModelRegistry::new());
    let providers = Arc::new(ProviderRegistry::new());

    let adapter = Arc::new(
        OpenAiCompatibleAdapter::new(
            "groq-api",
            "Mock Groq Inference",
            upstream_url,
            Some("mock_groq_key_val".to_string()),
        )
        .with_models(vec![ModelMetadata {
            model_id: "fast-chat-v1".to_string(),
            display_name: "Llama 3.3 70B Instruct (Groq)".to_string(),
            provider_id: "groq-api".to_string(),
            supported_modalities: vec!["text".to_string(), "code".to_string()],
            context_window: 131072,
            max_output_tokens: 8192,
            streaming_supported: true,
            tool_calling_supported: true,
            speed_tier: "fast".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec!["simple_chat".to_string(), "coding".to_string()],
            last_health_check: "2026-10-20T00:00:00Z".to_string(),
        }]),
    );
    providers.register(adapter).unwrap();

    let router = Arc::new(ModelRouter::new(registry.clone()));
    let state = AppState::with_all_components(config, registry, providers, router);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_app(state);

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{}", addr), handle)
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BenchmarkResult {
    endpoint: String,
    concurrency: usize,
    total_requests: usize,
    successful_requests: usize,
    failed_requests: usize,
    duration_secs: f64,
    rps: f64,
    min_ms: f64,
    avg_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    avg_ttfc_ms: Option<f64>,
}

#[allow(clippy::too_many_arguments)]
async fn run_benchmark_stage(
    client: reqwest::Client,
    base_url: &str,
    endpoint_name: &str,
    path: &str,
    method: &str,
    body: Option<serde_json::Value>,
    concurrency: usize,
    total_requests: usize,
    is_streaming: bool,
) -> BenchmarkResult {
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut tasks = Vec::with_capacity(total_requests);

    let start_wall = Instant::now();

    for _ in 0..total_requests {
        let sem = semaphore.clone();
        let client = client.clone();
        let url = format!("{}{}", base_url, path);
        let method = method.to_string();
        let body = body.clone();

        let task = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let req_start = Instant::now();

            let mut req_builder = match method.as_str() {
                "GET" => client.get(&url),
                "POST" => client.post(&url),
                _ => client.get(&url),
            };

            req_builder = req_builder
                .header("x-api-key", "test_load_api_key_secure_99")
                .header("content-type", "application/json");

            if is_streaming {
                req_builder = req_builder.header("accept", "text/event-stream");
            }

            if let Some(ref b) = body {
                req_builder = req_builder.json(b);
            }

            let resp = req_builder.send().await;
            match resp {
                Ok(response) => {
                    let status = response.status();
                    if is_streaming {
                        let mut stream = response.bytes_stream();
                        let mut first_chunk_time = None;
                        while let Some(chunk_res) = stream.next().await {
                            if chunk_res.is_ok() && first_chunk_time.is_none() {
                                first_chunk_time = Some(req_start.elapsed());
                            }
                        }
                        let total_dur = req_start.elapsed();
                        (
                            status.is_success(),
                            total_dur.as_secs_f64() * 1000.0,
                            first_chunk_time.map(|d| d.as_secs_f64() * 1000.0),
                        )
                    } else {
                        let _body_bytes = response.bytes().await;
                        let total_dur = req_start.elapsed();
                        (status.is_success(), total_dur.as_secs_f64() * 1000.0, None)
                    }
                }
                Err(_) => (false, req_start.elapsed().as_secs_f64() * 1000.0, None),
            }
        });
        tasks.push(task);
    }

    let mut latencies = Vec::with_capacity(total_requests);
    let mut ttfc_latencies = Vec::new();
    let mut success_count = 0;
    let mut fail_count = 0;

    for task in tasks {
        if let Ok((success, lat_ms, ttfc_opt)) = task.await {
            if success {
                success_count += 1;
                latencies.push(lat_ms);
                if let Some(ttfc) = ttfc_opt {
                    ttfc_latencies.push(ttfc);
                }
            } else {
                fail_count += 1;
            }
        } else {
            fail_count += 1;
        }
    }

    let total_wall = start_wall.elapsed().as_secs_f64();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let count = latencies.len();
    let min_ms = latencies.first().copied().unwrap_or(0.0);
    let max_ms = latencies.last().copied().unwrap_or(0.0);
    let avg_ms = if count > 0 {
        latencies.iter().sum::<f64>() / count as f64
    } else {
        0.0
    };
    let median_ms = if count > 0 { latencies[count / 2] } else { 0.0 };
    let p95_ms = if count > 0 {
        latencies[((count as f64 * 0.95) as usize).min(count - 1)]
    } else {
        0.0
    };
    let p99_ms = if count > 0 {
        latencies[((count as f64 * 0.99) as usize).min(count - 1)]
    } else {
        0.0
    };

    let avg_ttfc_ms = if !ttfc_latencies.is_empty() {
        Some(ttfc_latencies.iter().sum::<f64>() / ttfc_latencies.len() as f64)
    } else {
        None
    };

    let rps = if total_wall > 0.0 {
        success_count as f64 / total_wall
    } else {
        0.0
    };

    BenchmarkResult {
        endpoint: endpoint_name.to_string(),
        concurrency,
        total_requests,
        successful_requests: success_count,
        failed_requests: fail_count,
        duration_secs: total_wall,
        rps,
        min_ms,
        avg_ms,
        median_ms,
        p95_ms,
        p99_ms,
        max_ms,
        avg_ttfc_ms,
    }
}

#[tokio::test]
#[ignore = "Controlled load test: run via cargo test --test load_benchmark -- --ignored --nocapture"]
async fn test_safe_local_load_benchmark() {
    let (upstream_url, _upstream_h) = spawn_local_mock_upstream().await;
    let (backend_url, _backend_h) = setup_benchmark_backend(&upstream_url).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(30)
        .build()
        .unwrap();

    let concurrency_stages = vec![
        (1, 20),   // Stage 1: C=1, N=20
        (5, 50),   // Stage 2: C=5, N=50
        (10, 100), // Stage 3: C=10, N=100
        (20, 100), // Stage 4: C=20, N=100
    ];

    println!("\n================================================================================");
    println!("CONTROLLED LOCAL LOAD TEST & PERFORMANCE BENCHMARK (AI-BACKEND)");
    println!("Target: Local Axum Daemon on ephemeral port | Upstream: Local Mock Upstream");
    println!("Hardware: Intel Core i7-4510U (2 Cores, 4 Threads) | Max Concurrency: 20");
    println!("================================================================================\n");

    let mut all_results = Vec::new();

    // 1. Health Probe Benchmark
    println!("--> Benchmarking GET /api/v1/health (Liveness / Raw Axum Overhead)...");
    for &(concurrency, requests) in &concurrency_stages {
        let res = run_benchmark_stage(
            client.clone(),
            &backend_url,
            "GET /api/v1/health",
            "/api/v1/health",
            "GET",
            None,
            concurrency,
            requests,
            false,
        )
        .await;
        println!(
            "    [C={:02}, N={:03}] -> RPS: {:>7.1} | Avg: {:>6.2}ms | Med: {:>6.2}ms | P95: {:>6.2}ms | P99: {:>6.2}ms | Fail: {}",
            res.concurrency,
            res.total_requests,
            res.rps,
            res.avg_ms,
            res.median_ms,
            res.p95_ms,
            res.p99_ms,
            res.failed_requests
        );
        all_results.push(res);
    }

    // 2. Routing Preview Benchmark (Deterministic classifier + policy ranking, zero-network)
    println!("\n--> Benchmarking POST /api/v1/routing/preview (Classifier & Policy Engine)...");
    let preview_body = json!({
        "prompt": "Write a high-performance memory allocator in Rust using std::alloc.",
        "requested_modality": "code",
        "routing_mode": "auto"
    });
    for &(concurrency, requests) in &concurrency_stages {
        let res = run_benchmark_stage(
            client.clone(),
            &backend_url,
            "POST /api/v1/routing/preview",
            "/api/v1/routing/preview",
            "POST",
            Some(preview_body.clone()),
            concurrency,
            requests,
            false,
        )
        .await;
        println!(
            "    [C={:02}, N={:03}] -> RPS: {:>7.1} | Avg: {:>6.2}ms | Med: {:>6.2}ms | P95: {:>6.2}ms | P99: {:>6.2}ms | Fail: {}",
            res.concurrency,
            res.total_requests,
            res.rps,
            res.avg_ms,
            res.median_ms,
            res.p95_ms,
            res.p99_ms,
            res.failed_requests
        );
        all_results.push(res);
    }

    // 3. Chat Completions Benchmark (End-to-End pipeline with local mock provider)
    println!("\n--> Benchmarking POST /api/v1/chat/completions (Full Buffered Pipeline)...");
    let chat_body = json!({
        "messages": [
            { "role": "user", "content": "Explain Tokio runtime architecture." }
        ],
        "routing_mode": "auto"
    });
    for &(concurrency, requests) in &concurrency_stages {
        let res = run_benchmark_stage(
            client.clone(),
            &backend_url,
            "POST /api/v1/chat/completions",
            "/api/v1/chat/completions",
            "POST",
            Some(chat_body.clone()),
            concurrency,
            requests,
            false,
        )
        .await;
        println!(
            "    [C={:02}, N={:03}] -> RPS: {:>7.1} | Avg: {:>6.2}ms | Med: {:>6.2}ms | P95: {:>6.2}ms | P99: {:>6.2}ms | Fail: {}",
            res.concurrency,
            res.total_requests,
            res.rps,
            res.avg_ms,
            res.median_ms,
            res.p95_ms,
            res.p99_ms,
            res.failed_requests
        );
        all_results.push(res);
    }

    // 4. SSE Chat Stream Benchmark (End-to-End SSE streaming)
    println!("\n--> Benchmarking POST /api/v1/chat/stream (SSE Streaming Pipeline)...");
    let stream_body = json!({
        "messages": [
            { "role": "user", "content": "Stream advantages of Rust." }
        ],
        "routing_mode": "auto",
        "stream": true
    });
    for &(concurrency, requests) in &concurrency_stages {
        let res = run_benchmark_stage(
            client.clone(),
            &backend_url,
            "POST /api/v1/chat/stream",
            "/api/v1/chat/stream",
            "POST",
            Some(stream_body.clone()),
            concurrency,
            requests,
            true,
        )
        .await;
        let ttfc_str = res
            .avg_ttfc_ms
            .map(|t| format!("{:>5.2}ms", t))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "    [C={:02}, N={:03}] -> RPS: {:>7.1} | TTFC: {} | Avg: {:>6.2}ms | Med: {:>6.2}ms | P95: {:>6.2}ms | Fail: {}",
            res.concurrency,
            res.total_requests,
            res.rps,
            ttfc_str,
            res.avg_ms,
            res.median_ms,
            res.p95_ms,
            res.failed_requests
        );
        all_results.push(res);
    }

    println!("\n================================================================================");
    println!("LOAD TEST SUMMARY TABLE (FOR DOCUMENTATION)");
    println!("================================================================================");
    println!(
        "| Endpoint | Concurrency | Requests | Success | Fail | RPS | Avg Latency | Med Latency | P95 Latency | P99 Latency |"
    );
    println!("| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |");
    for r in &all_results {
        println!(
            "| `{}` | {} | {} | {} | {} | {:.1} | {:.2}ms | {:.2}ms | {:.2}ms | {:.2}ms |",
            r.endpoint,
            r.concurrency,
            r.total_requests,
            r.successful_requests,
            r.failed_requests,
            r.rps,
            r.avg_ms,
            r.median_ms,
            r.p95_ms,
            r.p99_ms
        );
    }
    println!("================================================================================\n");

    // Assert that zero requests failed across all stages and endpoints
    let total_failures: usize = all_results.iter().map(|r| r.failed_requests).sum();
    assert_eq!(
        total_failures, 0,
        "All benchmark requests across all concurrency levels must succeed"
    );
}
