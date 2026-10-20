# PHASE 10 — PERFORMANCE BASELINE REPORT

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](../MASTER_SPEC.md) | [`ARCHITECTURE.md`](../ARCHITECTURE.md)  
**Status:** Baseline Established (Pre-Optimization Measurements Recorded)  
**Date:** 2026-10-20  
**Initial Commit:** `2ce7b3707906baf482f3edde5552f4a10bd86f73`  

---

## 1. Executive Summary

Phase 10 begins with an evidence-driven, reproducible performance baseline measuring the existing Rust backend across routing, inference dispatch, streaming, raw Axum HTTP overhead, and token-handling characteristics.

Prior to implementing any token-related optimizations, two independent benchmark profiles were executed:
1. **Debug Profile (`dev` - unoptimized + debuginfo, codegen-units = 1)**
2. **Release Profile (`release` - fully optimized, LTO / high opt-level)**

Both profiles executed the controlled in-process load harness in `tests/load_benchmark.rs` against an ephemeral Axum server and local mock OpenAI-compatible upstream across 4 concurrency tiers ($C \in \{1, 5, 10, 20\}$) with bounded requests ($N \in \{20, 50, 100, 100\}$).

### Key Baseline Takeaways
- **Zero Failed Requests:** 0 errors across all 270 requests per run (540 total benchmark requests).
- **Peak Throughput:**
  - `GET /api/v1/health`: **5,455.2 RPS** (Release, $C=5$) / **1,124.8 RPS** (Debug, $C=20$).
  - `POST /api/v1/routing/preview`: **3,876.5 RPS** (Release, $C=20$) / **1,052.4 RPS** (Debug, $C=5$).
  - `POST /api/v1/chat/completions`: **1,161.1 RPS** (Release, $C=20$) / **295.1 RPS** (Debug, $C=5$).
  - `POST /api/v1/chat/stream`: **1,212.2 RPS** (Release, $C=10$) / **323.6 RPS** (Debug, $C=20$).
- **Sub-2ms Baseline Median Latencies ($C=1$, Release):**
  - Health check: **0.18 ms** (Release) / **1.52 ms** (Debug)
  - Routing preview: **0.24 ms** (Release) / **1.82 ms** (Debug)
  - End-to-end buffered completion: **1.45 ms** (Release) / **4.49 ms** (Debug)
  - Streaming Time-to-First-Chunk (TTFC): **1.44 ms** (Release) / **4.55 ms** (Debug)

---

## 2. Benchmark Environment & Methodology

### 2.1 Hardware and Runtime
- **Operating System:** Windows 11 x86_64
- **Processor:** Intel(R) Core(TM) i7-4510U CPU @ 2.00GHz (2 physical cores, 4 logical threads)
- **Memory:** DDR3 Dual-Channel
- **Rust Toolchain:** `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`
- **Dependencies:** Tokio 1.53.3, Axum 0.8.9, Reqwest 0.12, Rusqlite 0.33, Serde 1.0

### 2.2 Methodology and Test Isolation
- Upstream inference is serviced by an in-process local mock upstream TCP listener simulating OpenAI chat completion and SSE streaming endpoints.
- No external network requests or paid provider tokens are consumed during benchmarking.
- The standard unit test suite contains 196 tests passing across 10 test files. The load benchmark suite (`tests/load_benchmark.rs`) is marked `#[ignore]` so standard `cargo test` remains instant (~2.1s).
- Bounded concurrency stages:
  - Stage 1: $C=1, N=20$
  - Stage 2: $C=5, N=50$
  - Stage 3: $C=10, N=100$
  - Stage 4: $C=20, N=100$

### 2.3 Commands Executed
```powershell
# Debug Profile Benchmark
cargo test --test load_benchmark -- --ignored --nocapture

# Release Profile Benchmark
cargo test --release --test load_benchmark -- --ignored --nocapture

# Unit and Regression Verification
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

---

## 3. Measured Baseline Results

### 3.1 Release Profile Performance (`cargo test --release`)

| Endpoint | Concurrency ($C$) | Requests ($N$) | Success | Fail | Throughput (RPS) | Avg Latency | Median (p50) | p95 Latency | p99 Latency | TTFC |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| `GET /api/v1/health` | 1 | 20 | 20 | 0 | **2,478.3** | 0.39 ms | 0.18 ms | 2.36 ms | 2.36 ms | - |
| `GET /api/v1/health` | 5 | 50 | 50 | 0 | **5,455.2** | 0.77 ms | 0.48 ms | 2.37 ms | 2.77 ms | - |
| `GET /api/v1/health` | 10 | 100 | 100 | 0 | **2,798.6** | 3.29 ms | 2.06 ms | 10.26 ms | 10.52 ms | - |
| `GET /api/v1/health` | 20 | 100 | 100 | 0 | **3,031.8** | 5.70 ms | 4.11 ms | 13.68 ms | 15.15 ms | - |
| `POST /api/v1/routing/preview` | 1 | 20 | 20 | 0 | **2,281.1** | 0.43 ms | 0.24 ms | 3.48 ms | 3.48 ms | - |
| `POST /api/v1/routing/preview` | 5 | 50 | 50 | 0 | **3,542.8** | 1.25 ms | 0.99 ms | 2.99 ms | 3.08 ms | - |
| `POST /api/v1/routing/preview` | 10 | 100 | 100 | 0 | **3,296.4** | 2.55 ms | 2.18 ms | 4.73 ms | 5.18 ms | - |
| `POST /api/v1/routing/preview` | 20 | 100 | 100 | 0 | **3,876.5** | 4.24 ms | 3.97 ms | 6.89 ms | 7.08 ms | - |
| `POST /api/v1/chat/completions` | 1 | 20 | 20 | 0 | **359.9** | 2.77 ms | 1.45 ms | 19.77 ms | 19.77 ms | - |
| `POST /api/v1/chat/completions` | 5 | 50 | 50 | 0 | **674.6** | 7.08 ms | 6.98 ms | 10.20 ms | 10.31 ms | - |
| `POST /api/v1/chat/completions` | 10 | 100 | 100 | 0 | **540.7** | 18.17 ms | 13.09 ms | 34.14 ms | 34.17 ms | - |
| `POST /api/v1/chat/completions` | 20 | 100 | 100 | 0 | **1,161.1** | 16.48 ms | 17.62 ms | 23.88 ms | 23.95 ms | - |
| `POST /api/v1/chat/stream` | 1 | 20 | 20 | 0 | **677.9** | 1.46 ms | 1.25 ms | 3.42 ms | 3.42 ms | **1.44 ms** |
| `POST /api/v1/chat/stream` | 5 | 50 | 50 | 0 | **1,039.0** | 4.69 ms | 4.35 ms | 8.03 ms | 8.09 ms | **4.48 ms** |
| `POST /api/v1/chat/stream` | 10 | 100 | 100 | 0 | **1,212.2** | 8.04 ms | 8.34 ms | 12.00 ms | 12.05 ms | **7.77 ms** |
| `POST /api/v1/chat/stream` | 20 | 100 | 100 | 0 | **1,048.4** | 18.56 ms | 18.69 ms | 22.23 ms | 22.33 ms | **18.01 ms** |

### 3.2 Debug Profile Performance (`cargo test`)

| Endpoint | Concurrency ($C$) | Requests ($N$) | Success | Fail | Throughput (RPS) | Avg Latency | Median (p50) | p95 Latency | p99 Latency | TTFC |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| `GET /api/v1/health` | 1 | 20 | 20 | 0 | **418.2** | 2.32 ms | 1.52 ms | 9.91 ms | 9.91 ms | - |
| `GET /api/v1/health` | 5 | 50 | 50 | 0 | **331.1** | 11.75 ms | 5.06 ms | 69.45 ms | 79.38 ms | - |
| `GET /api/v1/health` | 10 | 100 | 100 | 0 | **645.4** | 13.50 ms | 10.58 ms | 29.72 ms | 39.43 ms | - |
| `GET /api/v1/health` | 20 | 100 | 100 | 0 | **1,124.8** | 15.31 ms | 14.38 ms | 23.51 ms | 25.55 ms | - |
| `POST /api/v1/routing/preview` | 1 | 20 | 20 | 0 | **454.4** | 2.12 ms | 1.82 ms | 6.56 ms | 6.56 ms | - |
| `POST /api/v1/routing/preview` | 5 | 50 | 50 | 0 | **1,052.4** | 3.97 ms | 4.01 ms | 5.38 ms | 5.46 ms | - |
| `POST /api/v1/routing/preview` | 10 | 100 | 100 | 0 | **908.2** | 8.81 ms | 8.85 ms | 12.33 ms | 13.08 ms | - |
| `POST /api/v1/routing/preview` | 20 | 100 | 100 | 0 | **775.4** | 20.71 ms | 20.27 ms | 25.71 ms | 26.67 ms | - |
| `POST /api/v1/chat/completions` | 1 | 20 | 20 | 0 | **188.5** | 5.24 ms | 4.49 ms | 10.30 ms | 10.30 ms | - |
| `POST /api/v1/chat/completions` | 5 | 50 | 50 | 0 | **295.1** | 15.99 ms | 16.01 ms | 18.81 ms | 19.24 ms | - |
| `POST /api/v1/chat/completions` | 10 | 100 | 100 | 0 | **286.0** | 32.79 ms | 31.32 ms | 38.83 ms | 39.71 ms | - |
| `POST /api/v1/chat/completions` | 20 | 100 | 100 | 0 | **256.4** | 74.29 ms | 79.36 ms | 100.73 ms | 122.83 ms | - |
| `POST /api/v1/chat/stream` | 1 | 20 | 20 | 0 | **209.4** | 4.70 ms | 4.28 ms | 9.49 ms | 9.49 ms | **4.55 ms** |
| `POST /api/v1/chat/stream` | 5 | 50 | 50 | 0 | **257.7** | 18.69 ms | 18.54 ms | 25.55 ms | 25.97 ms | **17.45 ms** |
| `POST /api/v1/chat/stream` | 10 | 100 | 100 | 0 | **266.0** | 36.15 ms | 35.41 ms | 51.71 ms | 52.86 ms | **33.99 ms** |
| `POST /api/v1/chat/stream` | 20 | 100 | 100 | 0 | **323.6** | 58.27 ms | 57.54 ms | 69.44 ms | 70.57 ms | **55.36 ms** |

---

## 4. Bottlenecks & Inefficiencies Identified by Evidence

Rigorous inspection of hot paths in `src/routes/chat.rs`, `src/inference/mod.rs`, `src/router/policy.rs`, and `src/providers/openai_compatible.rs` identified six specific bottlenecks:

### Bottleneck 1: Redundant Classification & Candidate Resolution in Chat Route
In `src/routes/chat.rs`:
```rust
let resolution_candidate = state.inference.resolve_candidates(&payload).ok();
match state.inference.execute_completion(&payload).await {
```
and within `state.inference.execute_completion(&payload)`:
```rust
req.validate()?;
let resolution = self.resolve_candidates(req)?;
```
`resolve_candidates` is executed **twice** on every single buffered completion request. The entire task classification pipeline (15-category regex evaluation, Hinglish/Roman Hindi parsing, SOV syntax checks, catalog querying, and candidate scoring) is executed on the first call, completely discarded if the request succeeds, and recalculated from scratch inside `execute_completion`.

### Bottleneck 2: Lack of Context Window Pre-Dispatch Validation
Neither `TaskClassifier` nor `InferenceService` checks whether the inbound request's token volume fits inside the candidate model's `context_window`. Models with 4,096 or 8,192 context windows could be selected for large prompts (e.g. 10,000–30,000 tokens), resulting in upstream provider rejection (HTTP 400/413/422) or context truncation, wasting bandwidth and triggering retry loops.

### Bottleneck 3: Complete Absence of Token Estimation
`UsageMetadata` in `src/models/mod.rs` defines `pub is_estimated: bool;`, but the codebase currently lacks any token estimation algorithm. When upstream providers omit usage metadata or prior to dispatch, token volume is completely unknown, preventing intelligent context window protection.

### Bottleneck 4: Redundant Message Content & Duplicate System Prompts
Chat clients and agent frameworks frequently repeat identical system instructions across conversation turns (e.g., repeating `"You are a helpful assistant."` consecutive times) or attach messages with empty leading/trailing padding. These consume unnecessary input tokens, inflating provider cost and reducing available context budget.

### Bottleneck 5: Excessive Allocations in the Candidate Retry Loop
In `InferenceService::execute_completion`:
```rust
let completion_req = CompletionRequest {
    model_id: candidate.model_id.clone(),
    messages: req.messages.clone(), // Clones all ChatMessages and String buffers
    ...
};
```
Every attempt and retry re-clones the entire `Vec<ChatMessage>` heap buffer.

### Bottleneck 6: Intermediate String Reallocations in SSE Chunk Parsing
In `src/providers/openai_compatible.rs::parse_sse_stream`:
```rust
let line: String = buffer.drain(..=pos).collect();
```
Calling `.drain(..=pos).collect::<String>()` performs memory shifting and allocates a brand new `String` on every single incoming SSE line.

---

## 5. Proposed Phase 10 Optimization Candidates

Based on the evidence, the following focused optimizations are proposed:

1. **Deterministic Token Estimator (`TokenEstimator` in `src/inference/tokens.rs`):**
   - Provide accurate character/word BPE approximation with role/primer framing tokens.
   - Implement thread-safe LRU caching for repeated prompt chunks to avoid redundant estimation.
   - Distinguish strictly between estimated tokens (`is_estimated: true`) and provider-reported usage (`is_estimated: false`).
2. **Context Window Protection & Capacity-Aware Routing:**
   - In `resolve_candidates`, estimate prompt tokens and disqualify/deprioritize candidates whose `context_window` cannot accommodate the prompt plus a completion buffer.
   - In manual mode, validate prompt tokens against the selected model's context window; return `AppError::InvalidRequest` early if prompt exceeds model capacity.
3. **Prompt & Message Compaction (`optimize_prompt_tokens`):**
   - Deduplicate consecutive identical system messages.
   - Strip exterior whitespace while preserving inner formatting, markdown, and code blocks.
   - Strictly preserve conversation turn order, semantic content, and user/assistant messages.
4. **Eliminate Redundant Routing in Chat Routes:**
   - Pass resolution details or execute directly without duplicate `resolve_candidates` invocations.
5. **Zero-Allocation SSE Chunk Slice Parsing:**
   - Parse SSE lines directly from buffer slices without intermediate `String` collection.

---

## 6. Measurements for Final Comparison

The final verification will compare:
1. Throughput (RPS) across concurrency levels $C=1, 5, 10, 20$.
2. Latency percentiles (p50, p95, p99) for completions, streaming TTFC, and routing preview.
3. Token savings from message compaction and prompt deduplication on multi-turn conversations.
4. Prevention of upstream failures for context-window-exceeding requests.
