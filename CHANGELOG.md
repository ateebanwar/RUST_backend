# CHANGELOG.md — Project Activity & Version History

All notable project changes, architectural decisions, and document synchronizations are recorded chronologically in this file.

## 2026-10-21 — Four-Gap Remediation, Adversarial Verification, and Production-Readiness Validation

### Added
- **Workstream A: Autonomous Retention & Retry Maintenance Worker (`src/usage/maintenance.rs`):**
  - Application-owned periodic Tokio maintenance loop executing at configured intervals (`USAGE_CLEANUP_INTERVAL_SECS`, default 3600s).
  - Progressive bounded batch deletion (up to 5 consecutive batches per tick, default 1000 records/batch) clearing large historical backlogs without locking SQLite.
  - Zero-noise execution (trace level on empty runs) and non-blocking passive WAL checkpoints.
  - Graceful cancellation handling via `tokio::sync::watch` channel integrated with Axum server termination.
- **Workstream B: Independent Local Disk Spooler (`src/usage/spool.rs`):**
  - Independent append-only disk spool (`data/usage_spool.log`) decoupled from SQLite.
  - Failsafe fallback: when SQLite storage and retry queues encounter write failures, records flush to the independent disk spool with sync flushing and atomic rewriting.
  - Strict capacity ceiling bounds (`USAGE_MAX_SPOOL_BYTES`, default 10MB) to prevent unbounded disk growth.
  - Automatic isolation of corrupted spool entries to `.dead_letter` file, preserving uncorrupted records.
  - Idempotent drain and restart recovery into SQLite upon database availability.
- **Workstream C: Multilingual Token Calibration & Context Safety (`src/inference/tokens.rs`):**
  - Extended tokenization heuristics for Roman Hinglish, Urdu/Arabic script, Devanagari, emojis, code indentation, and JSON tool schemas.
  - Introduced `EstimationMethod` and `TokenEstimate` with transparent approximation labeling (`is_exact: false`).
  - Added `estimate_request_context_safe` applying a conservative safety margin (+5% or at least +4 tokens) to strictly prevent context window overflow.
- **Workstream D: Security Hardening & Regression Testing (`src/security/mod.rs`):**
  - Hardened `HostValidator` rejecting malformed headers with control characters, CRLF, userinfo delimiters (`@`), commas, spaces, and path/query injections.
  - IPv6 normalization (`[::1]` -> `::1`) and wildcard subdomain matching (`*.example.com`) with lookalike domain rejection (`evil-example.com`, `notexample.com`).
  - Strict forward header stripping on direct requests without peer IP, preventing header spoofing.
  - Implemented `resolve_client_ip` parsing multi-hop `X-Forwarded-For` right-to-left to halt at the trusted proxy boundary.
- **Dedicated Regression Test Suites:**
  - `tests/retention_worker_test.rs`: 6 tests verifying progressive bounded pruning, boundary retention, empty databases, and shutdown.
  - `tests/durable_spool_test.rs`: 4 tests verifying independent spooling, restart recovery, dead-letter isolation, and capacity limits.
  - `tests/multilingual_token_safety_test.rs`: 7 tests verifying English, Hinglish, Urdu, emojis, code, tools JSON, and context budgets.
  - `tests/security_regression_test.rs`: 5 tests verifying Host validation, lookalike rejection, untrusted peer header rejection, and multi-hop XFF parsing.

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: 242 automated tests passing across 15 test suites with 0 failures and 0 errors.
- `cargo build --release`: Passed with exit code 0.
- Secret scan: 0 credentials or secrets committed across tracked files and git history.

---

## 2026-10-20 — Phase 10: Final Hardening, Correctness, Security, Token Validation, Production Readiness, and GitHub Release

### Added
- **Model-Aware Tokenizer Family Heuristics (`src/inference/tokens.rs`):** Added `TokenizerFamily` enum (`Llama`, `Mistral`, `Gemma`, `Qwen`, `DeepSeek`, `Gpt`, `Generic`) with model ID pattern matching and calibrated ratio scaling.
- **Collision-Safe Multi-Family Token Cache (`src/inference/tokens.rs`):** Replaced raw hash indexing with structured `CacheKey { text: String, family: TokenizerFamily }`, completely preventing false cache collisions across differing model families.
- **Strict Semantic-Preserving Prompt Compaction Engine (`src/inference/tokens.rs`):** Implemented conservative compaction that strictly protects leading Python code indentation, Markdown code fences (` ``` `, `~~~`), YAML/JSON configurations, shell commands, and SQL queries.
- **Granular Compaction Metrics:** Enhanced `TokenCompactionMetrics` with `original_characters`, `compacted_characters`, `bytes_saved`, `compaction_applied`, and `compaction_skipped_reason`.
- **Completion Allowance in Context Window Gating (`src/inference/mod.rs`):** Enforced pre-dispatch bounds checking incorporating requested `max_tokens` (`req.max_tokens.unwrap_or(256).min(model.max_output_tokens)`), preventing context truncation.
- **Network Security & Reverse-Proxy Subsystem (`src/security/mod.rs`):**
  - Bitmask `IpCidr` parsing supporting both IPv4 (0-32) and IPv6 (0-128) CIDR notations without arithmetic overflow panics.
  - `TrustedProxyMatcher` allowlisting proxy CIDRs.
  - `HostValidator` preventing HTTP Host-header poisoning.
  - Reverse-proxy middleware in `src/lib.rs` stripping spoofed `X-Forwarded-*` / `Forwarded` headers from untrusted network peers.
- **Durable Retention Pruning & WAL Checkpoint Maintenance (`src/usage/storage.rs`, `src/usage/service.rs`):** Bounded batch deletions (`prune_older_than_bounded`) avoiding SQLite database table locks, and passive WAL maintenance checkpoints (`maintenance_wal_checkpoint`).
- **Durable Usage Retry Queue (`src/usage/storage.rs`, `src/usage/service.rs`):** Added SQLite table `usage_retry_queue` with schema migration v2, exponential backoff (up to 300s), poison message isolation, and dead-letter queue metrics.
- **Git Hygiene & Secret Protection:** Hardened `.gitignore`, removed tracked `target/` binaries and `data/usage.db` from git index (`git rm --cached`), and added documented non-secret `.env.example` template.

### Hardened
- **SSE Stream Parser Chunking (`src/providers/openai_compatible.rs`):** Re-architected stream parser using raw byte buffers (`Vec<u8>`), completely eliminating multi-byte UTF-8 split boundary decoding errors across fragmented TCP packets.

### Verification
- `cargo fmt --check`: Clean formatting verified across all source and test files.
- `cargo check`: Clean compilation with 0 errors.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- `cargo test`: 220 automated tests passing (66 unit tests, 154 integration tests across 11 test suites) with 0 failures and 1 ignored load test.
- `cargo build --release`: Clean release compilation with 0 errors.
- `cargo test --release --test load_benchmark -- --ignored --nocapture`: Completed with 100% success rate across all 16 concurrency tiers.

---

## 2026-10-20 — Phase 10: Token Optimization & Performance Baseline Implementation

### Added
- **Deterministic Token Estimator (`src/inference/tokens.rs`):** Implemented high-performance, bounded, in-process token estimator conforming to 100k+ modern BPE vocabulary standards ($\max(1, \lceil\text{chars} / 4\rceil + \text{subword\_delimiters})$) with ChatML framing overhead (+4 per message, +3 primer). Supported by a thread-safe 512-slot circular LRU cache using 64-bit FNV-1a hashing.
- **Conservative Prompt Compaction Engine (`src/inference/tokens.rs`):** Implemented whitespace compaction preserving code indentation, consecutive duplicate system prompt deduplication, and compaction metrics tracking (`original_characters`, `compacted_characters`, `original_estimated_tokens`, `compacted_estimated_tokens`, `savings_percentage`).
- **Context Window Capacity-Aware Routing (`src/inference/mod.rs`):** Added pre-dispatch token capacity validation in `InferenceService`:
  - In `auto` mode: dynamically filters candidate models whose `context_window` cannot accommodate the prompt (+256 tokens completion headroom), preventing upstream context overflow rejections.
  - In `manual` mode: rejects prompts exceeding the requested model's context window immediately with canonical HTTP 400 `InvalidRequest` before initiating upstream network calls.
- **Dedicated Phase 10 Integration Test Suite (`tests/token_optimization_test.rs`):** 10 exhaustive integration tests covering estimation formula accuracy, ChatML framing, prompt compaction & whitespace preservation, thread-safe LRU cache hit/miss behavior, context window overflow rejection, auto-routing capacity filtering, and end-to-end HTTP buffered and SSE streaming execution.
- **Reproducible Performance Baseline & Comparison Reports:**
  - `docs/PHASE_10_PERFORMANCE_BASELINE.md`: Comprehensive empirical pre-optimization baseline across Debug and Release profiles under single-request ($C=1$) and concurrent ($C=20$) workloads.
  - `docs/PHASE_10_OPTIMIZATION_REPORT.md`: Full before-and-after empirical comparison report with statistical breakdown across p50, p95, p99 latencies, throughput, and token efficiency.

### Optimized
- **Redundant Routing Classification Elimination (`src/routes/chat.rs`):** Removed duplicate `state.inference.resolve_candidates` invocation on the happy path of `POST /api/v1/chat/completions`, isolating candidate resolution exclusively to the error path and yielding a 53.1% average latency reduction (0.98 ms -> 0.46 ms).
- **Zero-Allocation SSE Chunk Slice Parsing (`src/providers/openai_compatible.rs`):** Refactored `parse_sse_stream` to parse SSE line events directly from `&buffer[..pos]` string slices instead of allocating intermediate heap `String` objects per chunk, reducing memory churn and improving streaming TTFC by 31.6% (0.38 ms -> 0.26 ms at $C=20$).

### Verification
- `cargo fmt --check`: Clean formatting verified across all source and test files.
- `cargo check`: Clean compilation with 0 errors.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- `cargo test`: 206 automated tests passing across 11 test suites (+1 ignored load benchmark).
- `cargo build --release`: Clean release compilation with 0 errors.

---

## 2026-10-19 — Consolidated Audit Gap Repairs & Real Inference Implementation (Phase 7B)

### Added
- **Chat Completion & Streaming HTTP Endpoints (`src/routes/chat.rs`):** Resolved GAP-001 by implementing `POST /api/v1/chat/completions` (buffered synchronous completion) and `POST /api/v1/chat/stream` (SSE streaming protocol conforming to `API_SPEC.md` §5 & §6).
- **Concrete OpenAI-Compatible Provider Adapter (`src/providers/openai_compatible.rs`):** Resolved GAP-002 by implementing a full HTTP adapter supporting Groq Cloud (`https://api.groq.com/openai/v1`) and OpenRouter (`https://openrouter.ai/api/v1`) with connection pooling, bearer token auth, model wire mapping, custom headers, timeout enforcement, line-by-line SSE chunk unfolding, and sanitized error mapping.
- **End-to-End Inference Orchestration Service (`src/inference/mod.rs`):** Resolved GAP-003 by connecting the intelligent router directly to provider adapters. Pipeline: `Request Validation -> Task Classification -> Model Routing -> Provider Resolution -> Bounded Failover Execution -> Normalized Response & Streaming`.
- **Bounded Failover Policy Engine (`src/inference/mod.rs`):** Resolved GAP-005 by implementing bounded retries (ceiling of 3 candidate attempts), non-retryable error short-circuiting (400 `InvalidRequest`, 422 `CapabilityMismatch` do not trigger futile failover), and normalized 502 error when providers are exhausted.
- **Request ID Middleware & Propagation (`src/lib.rs`, `src/errors.rs`):** Resolved GAP-006 by assigning unique `req_<counter>` IDs or preserving valid client-supplied `x-request-id` headers, propagating them to both HTTP response headers and JSON error envelopes.
- **Constant-Time API Key Authentication Middleware (`src/lib.rs`, `src/config.rs`):** Resolved GAP-007 by validating `Authorization: Bearer <key>` and `X-API-Key: <key>` headers using constant-time comparison, while allowing public probes (`/api/v1/health`, `/api/v1/ready`) to bypass auth.
- **Provider Metadata Hygiene & Secret Redaction (`src/models/mod.rs`, `src/config.rs`):** Resolved GAP-010 by annotating `ProviderMetadata::base_url` with `#[serde(skip_serializing)]` to prevent leaking internal provider URLs to public callers, and implementing custom `fmt::Debug` for `AppConfig` with `[REDACTED]` values.
- **Production Security Hardening & Zero Unsafe Access (`src/config.rs`):** Enforced that in `Environment::Production`, authentication is unconditionally required on all protected endpoints (`/api/v1/chat/completions`, `/api/v1/chat/stream`, `/api/v1/models`, `/api/v1/routing/preview`); missing API key in production environment immediately rejects requests with HTTP 401 Unauthorized, eliminating unsafe public access while allowing probe endpoints (`/api/v1/health`, `/api/v1/ready`).
- **Gemini Configuration Mismatch Resolution (`src/config.rs`, `PROVIDER_INTEGRATION.md` §5):** Resolved status of `GEMINI_API_KEY`, documenting that the key is isolated in configuration schema and reserved for a future multimodal phase, while `groq-api` and `openrouter-api` serve as the active v1 execution adapters.
- **Dedicated Inference Integration Test Suite (`tests/chat_inference_test.rs`):** Resolved GAP-008 by adding 23 comprehensive end-to-end integration tests using in-process ephemeral TCP mock upstream servers, testing completion, SSE streaming chunks, rate limiting, timeouts, authentication, request ID propagation, failover bounds, invalid config rejections, production security controls, malformed response handling, 4.5 MB request body limits, mid-stream connection interruptions, client disconnect cancellation with synchronization signals, pre-stream failovers, prevention of partial output fallback, and secret leakage prevention.
- **Scoring Documentation & Weights Reconciliation (`ARCHITECTURE.md` §2.1, `tests/chat_inference_test.rs`):** Resolved GAP-011 by explicitly documenting the mathematical scoring formula, complexity weights, speed weights, capacity scores, and tie-breaking tiers in the architectural specification and testing mathematical determinism.
- Total automated tests expanded to 144 tests passing across 8 test suites with zero Clippy warnings.

---

## 2026-10-19 — Phase 7: Intelligent Model Router Implementation & Verification

### Added
- **Dynamic Multi-Criteria Scoring Policy Engine (`src/router/policy.rs`):** Implemented `RoutingPolicy` and `ScoredCandidate` conforming strictly to `MASTER_SPEC.md` §6, §18 Phase 7, `ARCHITECTURE.md` §2, and `DEC-010`:
  - **Hard Modality Prerequisite:** Strict verification of `supported_modalities` prior to scoring; incompatible models are completely excluded from both selected and fallback positions.
  - **Capability Matching (0–50 pts):** Weighted alignment with classified `TaskCategory` (coding, deep_reasoning, fast_reasoning, complex_analysis, simple_chat, general_conversation, document_creation, powerpoint_generation, data_extraction, image_generation, image_editing, video_generation, audio_processing, web_search).
  - **Complexity Alignment (0–30 pts):** High complexity prioritizes `advanced` speed tiers; low complexity prioritizes `fast` tiers and penalizes expensive models to protect token budgets.
  - **Speed & Latency Tier (0–15 pts):** Reward low-latency tiers (`fast`, `balanced`, `advanced`) contextually based on task complexity.
  - **Cost Tier Preference (0–15 pts):** Strict preference for `free-tier` (15 pts) and `low-cost` (10 pts) over `premium` (2 pts) per `MASTER_SPEC.md` §2.9.
  - **Capacity Evaluation (0–10 pts):** Context window (>= 64k) and max output tokens (>= 8k) factored into high/medium complexity scoring.
- **Deterministic Multi-Stage Tie-Breaking:** Resolves equal candidate scores deterministically: cost tier preference -> speed tier preference -> context window capacity -> alphabetical `model_id`.
- **Transparent Explainable Routing Metadata:** Generates human-readable `selection_reason` explaining task detection, capability tags, and efficiency justification.
- **Automatic vs. Manual Routing Handling:**
  - `manual` mode validates preferred model existence (HTTP 404), operational availability (HTTP 502), and modality compatibility (HTTP 422 `UNSUPPORTED_MODALITY`), ranking remaining available models as fallbacks.
  - `auto` mode dynamically scores all available catalog models and produces optimal selection with ranked `fallback_model_ids`.
- **Exhaustion & Unsupported Modality Error Envelopes:**
  - Returns HTTP 422 `UNSUPPORTED_MODALITY` if no model in the catalog supports the requested modality.
  - Returns HTTP 502 `UPSTREAM_FAILURE` if all models supporting the modality are currently marked unavailable.
- **Dedicated Phase 7 Test Suite (`tests/intelligent_router_test.rs`):** 16 comprehensive integration tests covering multi-criteria scoring across categories, auto vs. manual modes, modality hard filtering, unavailable model bypass, tie-breaking determinism, and multimodal catalog routing.
- Total automated tests expanded to 120 tests passing across 7 test suites with zero Clippy warnings.

---

## 2026-10-19 — Phase 6: Final Gap Verification and Closure

### Added
- **Roman Hindi & Hinglish SOV Pattern Support (`src/router/classifier.rs`):** Expanded task classification heuristics to handle Hindi/Urdu/Hinglish Subject-Object-Verb (SOV) grammatical patterns across all 15 categories, including:
  - PowerPoint/Slides: `"slides bana"`, `"presentation bana"`, `"ppt bana"`, `"deck bana"`.
  - Document Creation: `"document bana"`, `"report bana"`, `"contract draft"`, `"whitepaper likho"`.
  - Video Generation: `"video generate"`, `"video bana"`, `"animation bana"`.
  - Image Editing: `"background remove/hata"`, `"photo/image edit"`, `"crop/retouch photo"`.
  - Image Generation: `"image generate/bana"`, `"tasveer bana"`, `"photo bana"`.
  - Audio Processing: `"audio transcribe"`, `"audio sunke"`, `"awaz ko text"`, `"transcription karo"`.
  - Architecture: `"architecture design/bana"`, `"system design karo"`.
  - Coding & Debugging: `"code likho/bana"`, `"function "`, `"algorithm"`, `"bug/error fix"`.
  - Mathematics: `"calculate karo"`, `"hisaab karo"`, `"solve karo"`, `"equation solve"`.
  - Web Search: `"search karo"`, `"internet pe search"`, `"online dhoondo"`, `"taza khabar"`.
  - Data Extraction: `"json me convert"`, `"csv parse"`, `"table banao"`, `"data extract"`.
  - Complex Analysis: `"step by step explain"`, `"analyze karo"`, `"deep analysis"`, `"kyu hota hai explain"`.
  - Fast Reasoning vs Simple Questions: `"kya hai"`, `"kya hota hai"`, `"kaise kaam karta hai"` with domain-specific exclusion guards to prevent stealing math/search queries.
- **Dedicated Hinglish Integration Suite (`tests/task_classifier_test.rs`):** Added `test_roman_hindi_hinglish_classification` covering 22 diverse Hinglish prompts across all 15 categories.
- **Conflicting Signals and Ambiguous Cues Test Suite (`tests/task_classifier_test.rs`):** Added `test_conflicting_and_ambiguous_cues` validating modality precedence and graceful low-complexity fallback.
- **Empirical In-Process Microbenchmark (`tests/task_classifier_test.rs`):** Added `test_task_classifier_performance_benchmark` executing 10,000 iterations over a 10-prompt multilingual benchmark suite:
  - Median latency (p50): ~30 µs (0.030 ms)
  - 95th percentile (p95): ~69 µs (0.069 ms)
  - 99th percentile (p99): ~90 µs (0.090 ms)
  - Throughput: ~31,486 ops/sec (single-threaded CPU in-process microbenchmark)
  - Zero allocations/locks in hot path, zero network calls.
- Total automated tests increased from 101 to 104 across 6 test suites with zero warnings.

---

## 2026-10-19 — Phase 6: Task Classification Engine Implementation & Verification

### Added
- **Deterministic Task Classification Engine (`src/router/classifier.rs`):** Implemented high-performance, bounded, in-process classifier conforming strictly to `MASTER_SPEC.md` §6.1, §6.2, §6.3, §18 Phase 6, `API_SPEC.md` §4, and `ARCHITECTURE.md` §2 without external LLM calls or network dependencies.
- **Normalized Task Taxonomy (`TaskCategory`):** Fully typed enum supporting all 15 categories defined in the specification:
  - `SimpleQuestions` (`simple_questions`)
  - `GeneralConversation` (`general_conversation`)
  - `ComplexAnalysis` (`complex_analysis`)
  - `FastReasoning` (`fast_reasoning`)
  - `CodingAndDebugging` (`coding_and_debugging`)
  - `ArchitectureAndDesign` (`architecture_and_design`)
  - `MathematicsAndCalculation` (`mathematics_and_calculation`)
  - `WebSearchAndResearch` (`web_search_and_research`)
  - `ImageGeneration` (`image_generation`)
  - `ImageEditing` (`image_editing`)
  - `VideoGeneration` (`video_generation`)
  - `AudioProcessing` (`audio_processing`)
  - `DocumentCreation` (`document_creation`)
  - `PowerPointGeneration` (`powerpoint_generation`)
  - `DataExtraction` (`data_extraction_and_structuring`)
- **Task Complexity Tiers (`TaskComplexity`):** Fully typed enum (`Low`, `Medium`, `High`) mapped to model tier matching requirements per `MASTER_SPEC.md` §6.3.
- **Input & Output Modality Taxonomy (`TaskModality`):** Fully typed enum (`Text`, `Code`, `Image`, `Video`, `Audio`, `Document`) with strict string normalization and rejection of unsupported modalities (`AppError::InvalidRequest`).
- **Explainable Structured Output (`ClassificationResult`):** Provides transparent decision metadata including detected category, complexity tier, input/output modalities, tool requirement flags, deterministic confidence score (0.0 to 1.0), detected intent, and human-readable rationale.
- **Prompt Size Bounds & Input Validation:** Strict character-length validation using `chars().count()` enforcing the 1 to 100,000 character limit per `API_SPEC.md` §4.1, rejecting empty or whitespace-only prompts with canonical HTTP 400 envelopes.
- **Multilingual and Unicode Robustness:** UTF-8 safe parsing tested across English, Chinese, French, German, Arabic (supporting Arabic question mark `؟`), and multibyte emojis.
- **Router Integration & Consolidation (`src/router/mod.rs`):** Eliminated duplicate string-based heuristics; wired `ModelRouter::preview` to utilize structured classification with typed category matching and strict routing mode validation.
- **Dedicated Task Classification Test Suite (`tests/task_classifier_test.rs`):** 13 comprehensive integration tests covering all 15 categories, simple vs complex prompts, explicit modality overrides, mixed-intent resolution, determinism across 1,000 runs, JSON serialization/deserialization, size limit enforcement, parameter validation, and health/readiness non-regression.
- Total automated tests expanded to 101 tests passing across 6 test suites with zero Clippy warnings.

### Changed
- Reconciled Phase 5 status in `PROGRESS.md`: marked Phase 5 as "Approved by User" and Phase 6 as "Verified (Ready for Review)".
- Re-exported classification types (`TaskCategory`, `TaskComplexity`, `TaskModality`, `ClassificationResult`, `TaskClassifier`, `MAX_PROMPT_LENGTH`) from root `ai_backend` crate in `src/lib.rs`.

---

## 2026-10-19 — Phase 5: Readiness Contract Audit, Correction, and Final Acceptance

### Added
- **`active_provider_ids` Filtering (`src/providers/registry.rs`):** Added method to query only currently active (enabled and operational) provider IDs, preventing disabled or offline providers from inflating operational model counts.
- **Inference Readiness Contract Enforcement (`src/routes/health.rs`, `src/lib.rs`):** Reconciled `/api/v1/ready` and `ReadyResponse` with `API_SPEC.md` §2.2 and `DEC-009`:
  - When zero operational providers or models are available: returns HTTP 503 Service Unavailable with `{"status":"not_ready","active_providers":0,"registered_models":0,"deployment_mode":"..."}`.
  - When at least one operational provider and operational model are available: returns HTTP 200 OK with `{"status":"ready","active_providers":N,"registered_models":M,"deployment_mode":"..."}`.
- **Architecture Decision Record DEC-009 (`DECISIONS.md`):** Formally established the boundary between process liveness (`GET /api/v1/health`, HTTP 200) and operational inference readiness (`GET /api/v1/ready`, HTTP 200 vs 503), preventing conflation between reference catalog metadata and live inference availability.
- **Readiness Specification Clarification (`API_SPEC.md` §2.2):** Documented concrete response schemas for both operational (HTTP 200, `status: "ready"`) and unready (HTTP 503, `status: "not_ready"`) states.
- **Dedicated Readiness Regression Test Suite (`tests/readiness_regression_test.rs`):** Added 7 exhaustive tests validating all 11 required scenarios: clean startup isolation, disabled/offline adapter isolation, dynamic state transitions (operational -> offline -> operational), multi-provider filtering, discovery failure resiliency, and liveness decoupling. Total test suite expanded to 88 passing tests across 5 test suites.

### Changed
- Updated `tests/health_test.rs`, `src/lib.rs`, and `tests/model_registry_test.rs` to assert HTTP 503 and `status: "not_ready"` in initial clean no-provider state, and HTTP 200 `status: "ready"` upon provider registration and model synchronization.

---

## 2026-10-19 — Phase 5: Central Model Registry Implementation & Verification

### Added
- **Authoritative Central Model Catalog (`src/registry/mod.rs`):** Implemented thread-safe `ModelRegistry` catalog as the single source of truth for model metadata across routing, client listing, and discovery synchronization.
- **Strict Model Metadata Validation (`src/registry/mod.rs`):** Implemented `validate_model` enforcing alphanumeric/safe characters for model IDs (1–128 chars), non-empty display names (<= 256 chars), non-empty provider IDs (<= 128 chars), at least one non-empty supported modality, context window > 0, max output tokens > 0 and <= context window, normalized speed tiers (`fast`, `balanced`, `advanced`), and normalized cost tiers (`free-tier`, `low-cost`, `premium`).
- **Registration & Lifecycle Operations (`src/registry/mod.rs`):** Added `register_model` (safe atomic upsert), `register_new_model` (enforces unique model ID, rejecting duplicates with `RegistryError::DuplicateModel`), `update_model` (safe update returning `RegistryError::ModelNotFound` if missing), `remove_model`, and `clear`.
- **Typed Error Taxonomy & Canonical Mapping (`src/registry/mod.rs`):** Implemented `RegistryError` (`ModelNotFound`, `InvalidModel`, `DuplicateModel`, `ProviderNotFound`) with automatic conversion (`From<RegistryError> for AppError`) mapping to HTTP 404 (NOT_FOUND) and HTTP 400 (INVALID_REQUEST).
- **Multi-Dimensional Query Filtering (`src/registry/mod.rs`):** Implemented `ModelFilter` supporting modality filtering, speed tier filtering, provider ID filtering, and available-only filtering, with composite multi-filter execution via `query_models`.
- **Availability State Tracking (`src/registry/mod.rs`):** Implemented `set_model_availability` for granular model availability toggling and `set_provider_models_availability` for bulk provider outages, automatically updating RFC3339 UTC `last_health_check` timestamps.
- **Provider Discovery Integration (`src/providers/registry.rs`):** Updated `sync_models_to_registry` to validate discovered provider models and gracefully report errors without corrupting existing catalog state.
- **HTTP Routing & Schema Wiring (`src/routes/models.rs`, `src/lib.rs`):** Integrated `GET /api/v1/models` with query parameter binding (`modality`, `speed_tier`, `provider`, `available_only`), `GET /api/v1/models/{model_id}` with canonical 404 error envelope on missing models, and `GET /api/v1/providers` reflecting dynamic active provider state with catalog fallback.
- **Phase 4 Readiness Discrepancy Resolution (`src/routes/health.rs`, `src/lib.rs`):** Reconciled operational model counts so that `/api/v1/ready` reports active models tied to live operational providers (returning 0 in clean production state where no live upstream adapters are active, and dynamically counting operational models when providers are registered).
- **Dedicated Model Registry Integration Test Suite (`tests/model_registry_test.rs`):** Added 14 comprehensive integration tests covering validation rules, duplicate prevention, query filtering, availability transitions, canonical HTTP errors, and dynamic discovery/readiness resolution. Total tests increased to 81 (47 unit, 34 integration across 3 test suites), all passing.

### Changed
- Marked Phase 5 as **COMPLETE** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).
- Transitioned Phase 6 (Task Classification) to **READY FOR APPROVAL** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).

### Verification
- `cargo fmt --check`: Clean formatting verified across all files (0 errors).
- `cargo check`: Clean compilation with 0 errors.
- `cargo test`: 81 tests passed (47 unit tests in `src/lib.rs`, 14 integration tests in `tests/model_registry_test.rs`, 11 integration tests in `tests/adapter_contracts.rs`, 9 integration tests in `tests/health_test.rs`, 0 failures).
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- Live HTTP server execution verified on ephemeral port 8899:
  - `/api/v1/health` returned 200 with healthy status and RFC3339 timestamp.
  - `/api/v1/ready` returned 200 with operational counts (`active_providers: 0, registered_models: 0`).
  - `/api/v1/models` returned 200 with 3 catalog models and valid JSON envelope.
  - `/api/v1/models/fast-chat-v1` returned 200 with model details.
  - `/api/v1/models/nonexistent-model` returned 404 with canonical error envelope.
  - `/api/v1/models?provider=groq-api` returned 200 with 2 filtered models.
  - `/api/v1/models?speed_tier=fast` returned 200 with 1 filtered model.
  - `/api/v1/models?modality=code` returned 200 with 2 filtered models.
  - `/api/v1/providers` returned 200 with provider list.
  - `/api/v1/unknown_route` returned 404 with canonical error envelope.

---

## 2026-10-19 — Phase 4: Provider Adapter Framework Implementation & Verification

### Added
- **Asynchronous `ProviderAdapter` Trait (`src/providers/mod.rs`):** Standardized trait defining vendor-independent operations: static metadata retrieval (`provider_info`), declared capability reporting (`capabilities`, `supports_capability`), dynamic model discovery (`discover_models`), standard buffered completion (`complete`), streaming chunk delivery (`stream`), and health reachability probe (`check_health`).
- **Normalized Streaming & Domain Types (`src/models/mod.rs`):** Implemented `StreamChunk` (delta, is_done, usage), `NormalizedUsage` type alias, and added 10-second default `timeout: Duration` to `CompletionRequest`.
- **Provider Lifecycle Manager (`src/providers/registry.rs`):** Implemented `ProviderRegistry` providing thread-safe adapter registration, provider lookup (`get`), enabled/disabled state enforcement (`ProviderDisabled`), dynamic operational status tracking (`operational`, `degraded`, `offline`), individual and batch concurrent health checks (`check_all_health`), and model discovery synchronization (`sync_models_to_registry`).
- **Deterministic Mock Provider Adapter (`src/providers/mock.rs`):** Implemented `MockProviderAdapter` with customizable capabilities, simulated latency/delays, timeout enforcement, streaming chunk emission (`futures_util::stream::iter`), health status simulation, and error simulation across all 11 error variants without external network calls.
- **Normalized Provider Error Taxonomy (`src/providers/mod.rs`):** Implemented `ProviderError` with 11 variants mapped directly to canonical `AppError` and HTTP status codes (400, 404, 422, 429, 502, 504), with strict secret sanitization preventing upstream credentials from leaking into client-facing messages.
- **Windows Build Hardening (`Cargo.toml`):** Configured `codegen-units = 1` for dev and test profiles, eliminating `.rcgu.o` file locking churn under Windows Defender.
- **Provider Adapter Integration Test Suite (`tests/adapter_contracts.rs`):** Added 11 integration tests verifying buffered completion, streaming chunks, capability mismatch enforcement, timeout deadline enforcement, model not found, registry lifecycle, dynamic health updates, model discovery synchronization, concurrent registration, and secret isolation. Total test count increased to 64 passing tests (44 unit, 20 integration across 2 test suites).

### Changed
- Marked Phase 4 as **COMPLETE** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).
- Transitioned Phase 5 (Central Model Registry) to **READY FOR APPROVAL** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).

### Verification
- `cargo fmt --check`: Clean formatting verified across all files (0 errors).
- `cargo check`: Clean compilation with 0 errors.
- `cargo test`: 64 tests passed (44 unit tests in `src/lib.rs`, 11 integration tests in `tests/adapter_contracts.rs`, 9 integration tests in `tests/health_test.rs`, 0 failures).
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- Live HTTP server execution verified on port 8898 (`/api/v1/health` 200, `/api/v1/ready` 200, `/api/v1/models` 200, `/api/v1/providers` 200, unknown route 404).

---

## 2026-10-19 — Phases 3–7: Asynchronous Axum Server, Provider Abstraction, Model Registry & Routing


### Added
- **Windows File-Lock Permanent Resolution (`.cargo/config.toml`):** Configured `incremental = false` to permanently eliminate intermediate `.rcgu.o` object file churn that caused Windows Defender (`MsMpEng.exe`) sharing violations (os error 32).
- **Asynchronous HTTP Server Integration (`src/main.rs`, `src/lib.rs`):** Integrated Axum (v0.8.9) with Tokio async multi-threaded runtime, non-blocking TCP socket binding, structured logging, and fallback error handling.
- **Provider Abstraction Framework (`src/providers/mod.rs`):** Implemented asynchronous `ProviderAdapter` trait, normalized `ProviderError` enum with 9 variants, and deterministic `MockProviderAdapter` with failure simulation.
- **Central Model Registry (`src/registry/mod.rs`, `src/routes/models.rs`):** Implemented thread-safe `ModelRegistry` pre-populated with reference models (`fast-chat-v1`, `deep-reasoner-v1`, `balanced-assistant-v1`), query filtering by modality, speed tier, and provider, `GET /api/v1/models`, `GET /api/v1/models/{model_id}`, and `GET /api/v1/providers`.
- **Intelligent Model Router & Task Classifier (`src/router/mod.rs`, `src/routes/routing.rs`):** Implemented `TaskClassifier` with systems programming and analytical heuristics, `ModelRouter` supporting automatic capability matching and manual routing with fallbacks, and `POST /api/v1/routing/preview`.
- **Expanded Integration Test Suite (`tests/health_test.rs`):** Added tests covering model catalog, provider discovery, and routing preview dry runs, bringing total test count to 44 passing tests (35 unit, 9 integration).

### Verification
- `cargo fmt --check`: Clean formatting verified across all files.
- `cargo check`: Clean compilation with 0 errors.
- `cargo test`: 44 tests passed (35 unit tests, 9 integration tests, 0 failures).
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- Live HTTP verification on port 8899: `/api/v1/health`, `/api/v1/ready`, `/api/v1/models`, `/api/v1/models/fast-chat-v1`, `/api/v1/providers`, and `/api/v1/routing/preview` all verified functional.

---

## 2026-10-19 — Phase 3: Rust Core Foundation Implementation

### Added
- **Typed Configuration Module (`src/config.rs`):** Implemented `AppConfig`, `Environment`, `DeploymentMode` with environment variable loading, bounds validation (port range, valid IP/localhost, body size limits between 1KB–10MB, upstream timeouts 1s–30s), and custom `Debug` implementation preventing secret leakage.
- **Centralized Error Taxonomy (`src/errors.rs`):** Implemented `AppError` with 10 canonical error codes mapped to HTTP 4xx/5xx status codes, retryable flags, message sanitization, and standardized JSON error envelope formatting matching `API_SPEC.md` §1.3.
- **Health & Readiness Endpoints (`src/routes/health.rs`, `src/routes/mod.rs`):** Implemented `HealthResponse` (`/api/v1/health`) and `ReadyResponse` (`/api/v1/ready`) matching `API_SPEC.md` §2.1 and §2.2 with RFC 3339 UTC timestamp generation.
- **Application State & Route Dispatcher (`src/lib.rs`):** Implemented `AppState` for shared configuration and `dispatch_route` for in-memory and integration routing.
- **Server Entry Point (`src/main.rs`):** Implemented startup sequence with configuration loading, validation, address binding (`TcpListener`), structured startup logging, and connection handling loop.
- **Automated Integration Test Suite (`tests/health_test.rs`):** Added 6 integration tests covering health endpoint contract, readiness endpoint contract, canonical 404 envelope, valid configuration acceptance, invalid configuration rejection, and zero-secret leakage.

### Changed
- Marked Phase 3 as **COMPLETE** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).
- Transitioned Phase 4 (Provider Adapter Framework) to **READY FOR APPROVAL** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).

### Verification
- `cargo fmt --check`: Clean formatting verified across all source and test files.
- `cargo check --offline`: Compilation passed with 0 errors.
- `cargo test --offline`: 26 tests passed (20 unit tests, 6 integration tests, 0 failures).
- `cargo clippy --offline --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- Live HTTP server execution verified locally on port 8899 (`GET /api/v1/health` 200 OK, `GET /api/v1/ready` 200 OK, `GET /api/v1/unknown` 404 canonical envelope).

---

## 2026-10-19 — Phase 2: Architecture & API Contracts Finalization

### Added
- **Concrete REST API Request & Response Schemas:** Defined exact JSON schemas, field constraints, nullability semantics, and HTTP status mappings for all 11 core endpoints across Health, Models, Routing, Chat, Google Drive Sync, and Usage/Quotas in [`API_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/API_SPEC.md).
- **Server-Sent Events (SSE) Wire Framing Protocol:** Codified exact streaming headers (`text/event-stream; charset=utf-8`, `Cache-Control: no-cache`), event framing rules (`event: <type>\ndata: <json>\n\n`), 6 standard event types (`routing`, `chunk`, `: keep-alive`, `usage`, `error`, `done`), client disconnect handling via TCP socket detection, and upstream cancellation in [`API_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/API_SPEC.md).
- **Standardized Error Taxonomy & Envelope:** Defined unified error envelope (`code`, `message`, `retryable`, `request_id`, `details`) with 10 canonical error codes mapped to HTTP 4xx/5xx statuses, preventing internal stack traces or secrets from leaking in [`API_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/API_SPEC.md).
- **API Versioning & Deprecation Policy:** Codified path-based versioning (`/api/v1`), breaking vs non-breaking change definitions, and `Deprecation` / `Sunset` HTTP response headers with 90-day grace periods in [`API_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/API_SPEC.md).
- **16-Module DAG & Interface Contracts:** Decomposed system architecture into 16 cohesive modules (`api`, `auth`, `config`, `security`, `providers`, `adapters`, `models`, `registry`, `router`, `quota`, `usage`, `failover`, `chat`, `drive`, `observability`, `errors`) with explicit inputs, outputs, errors, allowed direct dependencies, and security boundaries in [`ARCHITECTURE.md`](file:///d:/Rust%20Backend/ai-backend/ARCHITECTURE.md).

### Changed
- Marked Phase 2 as **COMPLETE** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).
- Transitioned Phase 3 (Rust Core Foundation) to **READY FOR APPROVAL** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).

### Security
- Re-verified zero-leak error handling: public error responses strictly strip stack traces, credentials, and internal system details.
- Codified security boundaries across all 16 modules, ensuring untrusted client inputs are validated and capped at 4.5 MB at ingress.
- Preserved zero-cost, zero-database constraint and Google Drive `drive.appdata` least-privilege synchronization.

### Verification
- `cargo fmt --check`: Formatting verified cleanly.
- `cargo check --offline`: Offline compilation passed with 0 errors.
- `cargo test --offline`: Unit and doc tests passed with 0 errors.
- `cargo clippy --offline --all-targets --all-features -- -D warnings`: Passed with 0 warnings.
- Documentation consistency audit across all 12 authoritative Markdown files verified.

---

## 2026-10-19 — Phase 1: Requirements Clarification & Threat Model Finalization

### Added
- **Finalized Trust Boundaries & Asset Profiles:** Formally categorized high-value assets (API keys, user tokens, Drive credentials, conversation data) and 4 security trust boundaries in [`THREAT_MODEL.md`](file:///d:/Rust%20Backend/ai-backend/THREAT_MODEL.md).
- **STRIDE Threat Matrix Sign-Off:** Fully evaluated 10 concrete threat scenarios (T-01 through T-10) with likelihood, impact, planned mitigations, and verification tests.
- **Nonfunctional Requirements (NFR) Baseline:** Codified performance (<10ms routing p95, <50ms metadata p95), memory (<64MB), and execution deadline bounds (<10s upstream timeout for Vercel 15s ceiling).

### Changed
- Marked Phase 1 as **COMPLETE** in [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md).
- Transitioned Phase 2 (Architecture & API Contracts) to **READY FOR APPROVAL**.

### Verification
- `cargo fmt --check`: Clean formatting verified.
- `cargo check --offline`: Offline compilation passed with 0 errors.
- `cargo test --offline`: Unit and doc tests passed with 0 errors.
- `cargo clippy`: Passed with 0 warnings (`-D warnings`).

---

## 2026-10-19 — Master Documentation Completion & Architectural Synchronization

### Added
- **Project Scaffolding:** Initialized local Cargo package `ai-backend` with `src/main.rs`, `src/lib.rs`, `tests/`, and minimal configuration.
- **Master Specification (`MASTER_SPEC.md`):** Complete functional specification with reconciled multi-client scope, zero-cost constraints, and 20-phase roadmap.
- **Agent Operating Rules (`AGENTS.md`):** Binding guidelines, security constraints, and phased development workflows for AI agents.
- **Roadmap Tracker (`PROGRESS.md`):** Full 20-phase progress table with prerequisites, deliverables, and verification gates.
- **System Architecture (`ARCHITECTURE.md`):** Detailed component decomposition, Mermaid request flows, multi-client adaptation matrix, and stateless state management design.
- **STRIDE Threat Model (`THREAT_MODEL.md`):** Comprehensive security assessment covering 10 concrete attack scenarios, risk ratings, mitigations, and verification tests.
- **API Specification (`API_SPEC.md`):** Full REST API contracts, concrete JSON schemas, SSE streaming wire protocol, standardized error envelopes, and authentication headers.
- **Provider Framework (`PROVIDER_INTEGRATION.md`):** Proposed async `ProviderAdapter` Rust trait, normalized `ProviderError` enum, data structures, and 12-step onboarding checklist.
- **Security Baseline (`SECURITY.md`):** Actionable security policies covering secret isolation, SSRF prevention, request limits, and Google Drive least privilege (`drive.appdata`).
- **Testing Strategy (`TESTING.md`):** Multi-layered testing strategy, Wiremock-based provider mocking framework, and quality gate commands.
- **Deployment Guide (`DEPLOYMENT.md`):** In-depth technical evaluation of Vercel serverless execution limits, dual-target build strategy, and smoke test commands.
- **Architecture Decisions (`DECISIONS.md`):** 8 approved Architecture Decision Records (DEC-001 through DEC-008) resolving scope, hosting, auth, streaming, Drive sync, and crate selections.

### Changed
- **Scope Reconciliation:** Updated `MASTER_SPEC.md` to remove single-client Android/Flutter limitations, defining the backend as a decoupled, multi-client service serving Flutter mobile, a custom AI IDE, and editor integrations.
- **Hosting Reconciliation:** Reconciled the stateful Axum daemon description with Vercel's ephemeral serverless microVM limits, establishing a dual-target architecture with stateless heuristics and strict 10s upstream timeouts.
- **Google Drive Storage:** Established private `drive.appdata` scope as the approved storage location, protecting user chat history from accidental deletion while preserving privacy.

### Security
- Codified strict zero-client-secret rules: provider API keys are never sent to frontends or stored in client binaries.
- Established mandatory SSRF egress allowlisting blocking RFC 1918, loopback, and cloud metadata (`169.254.169.254`) ranges.
- Enforced 4.5 MB inbound request body size limit to match serverless microVM constraints and prevent memory exhaustion.
- Enforced structured log sanitization redacting tokens, keys, and raw user prompt text.

### Verification
- `cargo check`: Clean compilation with `rustc 1.99.1`.
- `cargo test`: Clean execution of library and binary test targets (0 errors).
- Verified existence and non-zero byte size of all 12 Markdown files on disk.

### Notes
- Documentation completion task only. No application business logic or backend features have been implemented.
- Phase 1 (Requirements & Threat Model) is unstarted and awaiting formal user authorization.
