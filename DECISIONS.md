# DECISIONS.md — Architecture Decision Records (ADR) Log

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Status:** Synchronized Architecture Decision Records  
**Last Updated:** 2026-10-19

---

## Decision Record Summary

| ADR ID | Decision Title | Status | Date | Primary Impact Area |
|---|---|---|---|---|
| **DEC-001** | Multi-Client Backend Architecture | **APPROVED** | 2026-10-19 | Core Scope & API Gateway |
| **DEC-002** | Dual-Target Deployment Model (Vercel Serverless + Daemon) | **APPROVED** | 2026-10-19 | Runtime & Infrastructure |
| **DEC-003** | Stateless Backend Core & Zero-Database Architecture | **APPROVED** | 2026-10-19 | State Management & Cost |
| **DEC-004** | Multi-Client Authentication Strategy (Google OIDC + PATs) | **APPROVED** | 2026-10-19 | Security & Identity |
| **DEC-005** | Streaming Protocol Selection: Server-Sent Events (SSE) | **APPROVED** | 2026-10-19 | API Protocol & Streaming |
| **DEC-006** | Google Drive `appDataFolder` Chat Synchronization | **APPROVED** | 2026-10-19 | Chat Persistence & Privacy |
| **DEC-007** | Standardized `ProviderAdapter` Async Trait & Error Mapping | **APPROVED** | 2026-10-19 | Provider Extensibility |
| **DEC-008** | Core Technology Stack: Axum, Tokio, Reqwest, Serde, Tracing | **APPROVED** | 2026-10-19 | Rust Engineering Baseline |
| **DEC-009** | Readiness Probe Semantics (Inference Readiness vs. Liveness) | **APPROVED** | 2026-10-19 | Operational Probes & Health |
| **DEC-010** | Multi-Criteria Intelligent Model Routing Policy & Scoring Matrix | **APPROVED** | 2026-10-19 | Model Selection & Fallback Ordering |
| **DEC-011** | Real HTTP Provider Adapter, Inference Pipeline Execution, and Bounded Failover Architecture | **APPROVED** | 2026-10-19 | Inference Execution & Resilience |
| **DEC-012** | Deterministic Token Estimation, Prompt Compaction, and Pre-Dispatch Context Window Routing Safeguards | **APPROVED** | 2026-10-20 | Token Efficiency & Context Capacity |
| **DEC-013** | Model-Aware Token Estimation Provenance, Conservative Prompt Preservation, and Reverse-Proxy / Durable Usage Resilience | **APPROVED** | 2026-10-20 | Security, Correctness & Reliability |
| **DEC-014** | Autonomous Retention Maintenance Worker, Independent Local Disk Spooler, Multilingual Token Provenance, and Reverse-Proxy Security Defense | **APPROVED** | 2026-10-21 | Reliability, Storage & Security |

---

### DEC-001: Multi-Client Backend Architecture
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** The initial draft specification described the project narrowly as an "Android application backend for Flutter", while requirements mandated supporting a future custom IDE and editor integrations.
- **Options Considered:**
  1. *Flutter-Only Monolithic Backend:* Hardcodes mobile-specific response shapes and authentication flows.
  2. *Decoupled Multi-Client API Gateway:* Core routing, adapters, and services are frontend-agnostic and serve Flutter, custom IDEs, VS Code extensions, and CLI tools.
- **Selected Option:** Option 2 (Decoupled Multi-Client API Gateway).
- **Rationale:** Prevents rewriting provider integrations and routing logic when introducing the custom IDE and editor extensions.
- **Consequences:** API schemas and auth headers must support both mobile (Google Sign-In) and developer (PAT/API key) workflows.

---

### DEC-002: Dual-Target Deployment Model (Vercel Serverless + Daemon)
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** The project has an existing Vercel deployment target and a zero-cost objective, but also describes a high-performance Axum/Tokio architecture. Vercel functions run as ephemeral AWS Lambda microVMs with a 15-second execution limit (Hobby tier).
- **Options Considered:**
  1. *Vercel-Only Serverless Monolith:* Binds code strictly to AWS Lambda / Vercel adapters.
  2. *Dedicated Container Only (Fly.io / Render / VPS):* Requires managing Docker containers and may incur hosting costs.
  3. *Dual-Target Core Architecture:* Code is built as a library crate (`lib.rs`) exposing an `axum::Router`. It compiles both as a standard daemon (`src/main.rs`) for local dev / Docker and a serverless function (`lambda_http::run`) for Vercel.
- **Selected Option:** Option 3 (Dual-Target Core Architecture).
- **Rationale:** Achieves $0 recurring cost on Vercel while preserving instant portability to containerized runtimes if long-running streaming or strict in-memory state tracking requires it.
- **Consequences:** All upstream requests must enforce a 10s timeout to fit within Vercel's 15s execution ceiling.

---

### DEC-003: Stateless Backend Core & Zero-Database Architecture
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** Requirements specify zero recurring infrastructure costs wherever feasible and no dedicated database in the initial architecture.
- **Options Considered:**
  1. *Provision a Managed Database (PostgreSQL / Supabase / Redis):* Adds potential recurring costs, credentials, and maintenance overhead.
  2. *Stateless Backend Core with Client-Managed Google Drive Persistence:* User conversations are saved in their own Google Drive; backend retains zero durable conversation state.
- **Selected Option:** Option 2 (Stateless Backend Core with Google Drive).
- **Rationale:** Zero infrastructure cost, maximum user privacy, and complete compatibility with ephemeral serverless runtimes.
- **Consequences:** Rate limiting and circuit breaking rely on fast-decay heuristics per warm instance; usage stats are returned to clients for local aggregation.

---

### DEC-004: Multi-Client Authentication Strategy (Google OIDC + PATs)
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** Mobile users require friction-free sign-in, while developers using custom IDEs or CLI tools cannot easily perform mobile OAuth redirects.
- **Options Considered:**
  1. *Google Sign-In Exclusively for All Clients:* Difficult for headless terminal tools and IDE extensions.
  2. *API Keys Only:* Inconvenient and less secure for mobile end-users.
  3. *Dual Authentication Scheme:* Google OIDC ID tokens for mobile/web; Personal Access Tokens (PATs) / API keys with constant-time verification for IDEs and extensions.
- **Selected Option:** Option 3 (Dual Authentication Scheme).
- **Rationale:** Delivers optimal user experience across mobile and developer developer tools without compromising security.
- **Consequences:** Auth middleware must inspect incoming headers and branch token validation accordingly.

---

### DEC-005: Streaming Protocol Selection: Server-Sent Events (SSE)
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** Real-time token streaming is required for interactive chat.
- **Options Considered:**
  1. *WebSockets:* Requires persistent bidirectional TCP connections, incompatible with standard serverless function lifecycles.
  2. *Server-Sent Events (SSE) via HTTP Chunked Transfer:* Simple unidirectional HTTP streaming supported natively by Axum, mobile HTTP clients, and Vercel serverless functions.
  3. *Raw Chunked NDJSON:* Functional, but lacks standard browser/mobile event framing (`event: chunk`, `event: done`).
- **Selected Option:** Option 2 (Server-Sent Events / SSE).
- **Rationale:** Native compatibility with HTTP/2 and serverless proxies, minimal overhead, and built-in event discrimination.
- **Consequences:** Client disconnects must be monitored by reading TCP socket state to abort upstream requests promptly.

---

### DEC-006: Google Drive `appDataFolder` Chat Synchronization
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** User chat history must persist across app reinstallation without a server-side database.
- **Options Considered:**
  1. *User-Visible Google Drive Folder (`drive.file`):* Files appear in user's general Google Drive, risking accidental user deletion or modification.
  2. *Private Application Data Folder (`drive.appdata`):* Files are stored in a dedicated, hidden space accessible only by the authorized application.
- **Selected Option:** Option 2 (`drive.appdata`).
- **Rationale:** Maximum least-privilege security; prevents the app from accessing user files and protects chat backups from accidental user tampering.
- **Consequences:** Requires the client to request `https://www.googleapis.com/auth/drive.appdata` during OAuth consent.

---

### DEC-007: Standardized `ProviderAdapter` Async Trait & Error Mapping
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** Multiple independent AI providers and open-source repositories must be integrated without polluting core routing logic.
- **Options Considered:**
  1. *Ad-hoc Client Functions per Provider:* Leads to tight coupling and scattered error handling.
  2. *Unified `ProviderAdapter` Trait with Strongly-Typed `ProviderError`:* Isolates vendor HTTP formats and error mapping behind a clean interface.
- **Selected Option:** Option 2 (Unified `ProviderAdapter` Trait).
- **Rationale:** Adding a future provider requires only implementing one trait and writing unit tests. Core router remains completely decoupled.
- **Consequences:** All provider streaming and completion responses must be mapped to normalized internal data structures.

---

### DEC-008: Core Rust Tech Stack Baseline
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** Foundational crate selections must be determined to ensure memory safety, performance, and small binary size.
- **Selected Baseline:**
  - **Runtime & Web Framework:** `tokio` (async runtime), `axum` (HTTP routing), `tower` / `tower-http` (middleware, CORS, trace).
  - **HTTP Client:** `reqwest` (with connection pooling, `rustls-tls` to avoid OpenSSL C dependencies).
  - **Serialization:** `serde`, `serde_json`.
  - **Diagnostics & Logging:** `tracing`, `tracing-subscriber`.
  - **Security:** `secrecy` (memory zeroizing), `subtle` (constant-time token comparison).
  - **Serverless Adapter:** `lambda_http` (for Vercel AWS Lambda compatibility).
- **Rationale:** Battle-tested, memory-safe, pure-Rust (via rustls), and optimal for small binary sizes and fast cold starts.
- **Consequences:** Excludes unvetted or C-dependent libraries.

---

### DEC-009: Readiness Probe Semantics (Inference Readiness vs. Liveness)
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** An inconsistency was identified where `GET /api/v1/ready` returned HTTP 200 OK with `status: "ready"` despite `active_providers: 0` and `registered_models: 0`. The architecture required an explicit specification distinguishing process-level health from operational inference readiness.
- **Options Considered:**
  1. *Process-Level Readiness Only:* `/api/v1/ready` returns 200 OK as long as the HTTP listener is up, regardless of whether upstream providers are configured or operational.
  2. *Conflating Reference Metadata with Operational Capacity:* Counting static/reference catalog models as operational to force readiness to return 200 OK.
  3. *Inference Readiness with Strict Decoupling:*
     - Liveness (`/api/v1/health`) checks process health and returns 200 OK independently of providers.
     - Readiness (`/api/v1/ready`) reflects operational capacity to serve inference requests. It returns HTTP 200 OK with `status: "ready"` only when at least one operational provider adapter is registered and operational models exist; otherwise, it returns HTTP 503 Service Unavailable with `status: "not_ready"`.
     - Reference models in `ModelRegistry` catalog remain queryable for UI and client preview without corrupting operational readiness metrics.
- **Selected Option:** Option 3 (Inference Readiness with Strict Decoupling).
- **Rationale:** Aligns with `API_SPEC.md` §2.2 (`200 OK (if at least one provider is ready) or 503 Service Unavailable`), prevents orchestrators and clients from routing inference requests to instances with zero operational providers, and prevents false operational status while maintaining catalog utility.
- **Consequences:** Systems with zero registered upstream providers correctly report 503 `not_ready` on `/api/v1/ready` while reporting 200 `healthy` on `/api/v1/health`.

---

### DEC-010: Multi-Criteria Intelligent Model Routing Policy & Scoring Matrix
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** `MASTER_SPEC.md` §6 and Phase 7 require selecting compatible models according to task requirements, capability, latency, reliability, and cost category. The engine must support automatic routing, manual selection, explainable decisions, and deterministic fallback ordering without fabricating telemetry.
- **Options Considered:**
  1. *Static Hardcoded Lookup Table:* Maps each task category directly to a fixed model ID string. Brittle, fails when models are added, removed, or unavailable.
  2. *Live Telemetry Polling with Fabricated Stats:* Attempts dynamic latency and pricing polling; risks fabricating metrics when providers do not expose real-time telemetry.
  3. *Multi-Criteria Scoring Engine with Catalog Introspection & Documented Safe Policy:*
     - Evaluates models dynamically from `ModelRegistry`.
     - Strict modality support is a non-negotiable prerequisite.
     - Multi-dimensional scoring evaluates capability match (0–50 pts), complexity alignment (0–30 pts), speed/latency tier (0–15 pts), cost tier (0–15 pts), and context capacity (0–10 pts).
     - Deterministic tie-breaking: cost tier preference, followed by speed tier, context window, and alphabetical `model_id`.
     - In manual mode: validates existence, availability, and modality compatibility; ranks remaining compatible models as fallbacks.
     - When no operational models match: returns canonical HTTP 422 (`UNSUPPORTED_MODALITY`) or HTTP 502 (`UPSTREAM_FAILURE`).
- **Selected Option:** Option 3 (Multi-Criteria Scoring Engine with Catalog Introspection).
---

### DEC-011: Real HTTP Provider Adapter, Inference Pipeline Execution, and Bounded Failover Architecture
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** An audit identified that while Phases 1–7 completed foundational interfaces, routing, and classification, live chat completions and streaming endpoints were unmapped (`GAP-001`), the provider adapter was mock-only (`GAP-002`), router-to-provider execution was disconnected (`GAP-003`), fallback failover was unbound (`GAP-005`), request ID propagation and auth were missing (`GAP-006`, `GAP-007`), and provider base URLs were exposed in public JSON responses (`GAP-010`).
- **Options Considered:**
  1. *External Mock Framework (Wiremock / Mockito):* Introduces heavy external JVM or complex crate dependencies that complicate local Windows builds and CI.
  2. *Integrated Pure-Rust Architecture:*
     - Implement concrete `OpenAiCompatibleAdapter` utilizing `reqwest` (with `rustls-tls` to avoid C OpenSSL dependencies).
     - End-to-end inference service (`InferenceService`) orchestrating `Validation -> Task Classification -> Model Routing -> Provider Resolution -> Bounded Failover Execution -> Normalized Response & Streaming`.
     - Standardize SSE streaming using `axum::response::sse::Sse` and `futures_util::stream::unfold`.
     - Implement request ID tracing and constant-time API authentication middleware.
     - Redact provider `base_url` via `#[serde(skip_serializing)]` from public responses.
     - Implement native in-process ephemeral `tokio::net::TcpListener` HTTP servers for zero-dependency integration testing.
- **Selected Option:** Option 2 (Integrated Pure-Rust Architecture).
- **Rationale:** Preserves zero external network leakage during testing, eliminates heavyweight dependencies, maintains small binaries, ensures secret isolation, and provides fully functional end-to-end inference execution conforming strictly to `MASTER_SPEC.md` §6 & §18, `API_SPEC.md` §5 & §6, and `SECURITY.md`.
- **Consequences:** Endpoints `POST /api/v1/chat/completions` and `POST /api/v1/chat/stream` are fully functional and tested against real HTTP adapters and ephemeral mock servers. Phase 8 (Usage and Quota Tracking) remains strictly on hold.

---

### DEC-012: Deterministic Token Estimation, Prompt Compaction, and Pre-Dispatch Context Window Routing Safeguards
- **Status:** APPROVED
- **Date:** 2026-10-20
- **Context:** To optimize token utilization, prevent model context window overflow before invoking remote provider APIs, and avoid redundant pipeline computation, Phase 10 required in-process token estimation, conservative compaction, capacity-aware routing, and parser optimizations without adding heavy external C-based tokenizers or altering semantic message content.
- **Options Considered:**
  1. *External Full Tokenizer Crate (e.g. tiktoken-rs / HuggingFace tokenizers):* Adds large precompiled BPE vocabularies (tens of megabytes of binary bloat) and potential C/Python dependencies, degrading binary size and serverless cold start times.
  2. *Heuristic Estimator with Thread-Safe LRU Caching and Semantic Guardrails:*
     - Pure Rust subword estimator conforming to modern 100k+ vocabulary standards: $\max(1, \lceil\text{chars} / 4\rceil + \text{subword\_delimiters})$ plus ChatML per-message framing overhead (+4 per message, +3 primer).
     - Thread-safe 512-slot circular LRU cache using 64-bit FNV-1a hashing for sub-microsecond amortized repeated prompt queries.
     - Conservative whitespace compaction preserving leading line indentation (crucial for code snippets) and deduplicating consecutive identical system prompts.
     - Capacity-aware routing in `InferenceService`: In `auto` mode, filters out candidate models whose `context_window` cannot fit the prompt (+256 tokens completion headroom). In `manual` mode, rejects oversized prompts immediately with HTTP 400 `InvalidRequest`.
     - Zero-allocation SSE chunk slice parsing in `src/providers/openai_compatible.rs`.
     - Elimination of duplicate candidate resolution in `src/routes/chat.rs`.
- **Selected Option:** Option 2 (Heuristic Estimator with Thread-Safe LRU Caching and Semantic Guardrails).
- **Rationale:** Zero external dependencies or binary bloat, negligible CPU overhead (<1 µs), prevents costly upstream 400 context overflow rejections, eliminates redundant routing classification, and delivers proven latency/throughput improvements (+112% RPS on completion, 53% latency reduction).
- **Consequences:** All chat completion and streaming pipelines automatically validate and filter by context capacity. Usage reporting preserves provider-reported token figures while internal routing relies on deterministic estimates for pre-dispatch gating.

---

### DEC-013: Model-Aware Token Estimation Provenance, Conservative Prompt Preservation, and Reverse-Proxy / Durable Usage Resilience
- **Status:** APPROVED
- **Date:** 2026-10-20
- **Context:** Following the initial Phase 10 optimization baseline, final hardening required resolving token estimator accuracy across model families without cache collisions, preventing Python/YAML formatting corruption during prompt compaction, securing the service against untrusted reverse-proxy forwarded headers and Host-header injection, avoiding long-lived database table locks during retention pruning, and establishing durable recovery for transient usage recording failures.
- **Decisions Made:**
  1. *Model-Aware Tokenizer Family & Provenance:* Introduced `TokenizerFamily` enum (`Llama`, `Mistral`, `Gemma`, `Qwen`, `DeepSeek`, `Gpt`, `Generic`) with model ID pattern matching. Replaced raw hash indexing with collision-safe `CacheKey { text, family }`. Token usage provenance explicitly separates estimated heuristics from provider-reported numbers; missing usage is never coerced to zero.
  2. *Strict Semantic-Preserving Prompt Compaction:* Compaction strictly preserves Python code indentation, Markdown code fences (` ``` `, `~~~`), YAML/JSON blocks, shell commands, and SQL queries. Restricts whitespace cleanup to collapsing 3+ blank lines to 2 and stripping trailing line spaces. Consecutive identical system instructions are deduplicated with granular metrics (`original_characters`, `compacted_characters`, `bytes_saved`, `compaction_applied`, `compaction_skipped_reason`).
  3. *Context Window Completion Allowance:* Factored in client requested `max_tokens` (`req.max_tokens.unwrap_or(256).min(model.max_output_tokens)`) into pre-dispatch gating, rejecting requests whose prompt + completion allowance exceeds model context window.
  4. *SSE Byte Buffer Chunking:* Switched `OpenAiCompatibleAdapter::parse_sse_stream` from string-based slicing to byte-level `Vec<u8>` chunk processing, splitting on `b'\n'` and stripping `b'\r'`. This guarantees that multi-byte UTF-8 code points split across TCP packet boundaries are never corrupted or malformed.
  5. *Network Security & Reverse-Proxy Hardening:* Implemented `IpCidr` bitmask parsing for IPv4 & IPv6 with shift-overflow protection for prefix 0. Added `TrustedProxyMatcher` and `HostValidator`. Strips spoofed `X-Forwarded-*` / `Forwarded` headers from untrusted clients, trusting headers only from explicitly configured trusted proxy CIDRs.
  6. *Bounded Retention Maintenance & WAL Checkpoints:* Added bounded subquery deletion (`prune_older_than_bounded`) to delete records in chunks (e.g. 1000 records) to prevent SQLite database write lock contention, coupled with passive WAL maintenance checkpoints (`maintenance_wal_checkpoint`).
  7. *Durable Usage Recording Retry Queue:* Added `usage_retry_queue` table with schema migration v2. On database persistence failure, usage records are serialized and enqueued for exponential backoff retries (up to 300s). Poison messages or exhausted retries (5 attempts) transition safely to dead-letter status, preserving user-visible inference semantics while ensuring accounting auditability.
  8. *Git Hygiene & Secret Protection:* Staged removal of tracked `target/` binaries and `data/usage.db` from git index (`git rm --cached`), hardened `.gitignore`, and added non-secret `.env.example` template.
- **Consequences:** All acceptance gates (Gate A through Gate J) are satisfied with 220 automated tests passing, 0 Clippy warnings, and clean release compilation. Phase 11 (Authentication) is eligible to begin next upon explicit user authorization.

---

### DEC-014: Autonomous Retention Maintenance Worker, Independent Local Disk Spooler, Multilingual Token Provenance, and Reverse-Proxy Security Defense
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** Remediating the four operational gaps required: (1) making retention cleanup an autonomous, lifecycle-managed background worker rather than a manual endpoint, (2) solving the SQLite single-file write lockout vulnerability by decoupling retries to an independent append-only disk spool with capacity ceilings, (3) providing model-aware multilingual token estimation with explicit provenance and conservative context safety margins, and (4) hardening Host validation, IPv6 normalization, and reverse-proxy multi-hop forwarded IP extraction.
- **Decisions Made:**
  1. *Autonomous Retention & Retry Maintenance Worker (`src/usage/maintenance.rs`):* Application-owned Tokio task running periodically (`usage_cleanup_interval_secs`, default 3600s). Processes expired records in progressive bounded batches (up to 5 consecutive batches per cycle, default 1000 records/batch) to clear large backlogs without monopolizing database locks. Responds immediately to application graceful shutdown signals (`tokio::sync::watch`) with a 5s drain deadline.
  2. *Independent Durable Disk Spool (`src/usage/spool.rs`):* If SQLite is locked, corrupted, or unavailable, failed usage records immediately bypass SQLite and append to an independent local spool file (`data/usage_spool.log`). Features atomic temporary file rewriting on drain, strict capacity ceiling enforcement (`usage_max_spool_bytes`, default 10MB), and automatic dead-letter isolation of corrupted lines (`.dead_letter`).
  3. *Multilingual Token Calibration & Context Safety (`src/inference/tokens.rs`):* Calibrated token counting across Roman Hinglish, Urdu/Arabic script, Devanagari, emojis, code indentation, and JSON tool schemas. Introduces `EstimationMethod` and `TokenEstimate` with transparent approximation labeling (`is_exact: false`). Applies a conservative safety margin (+5% or at least 4 tokens) in `estimate_request_context_safe` to strictly prevent context overflow on boundary requests.
  4. *Hardened Host & Forwarded-Header Validation (`src/security/mod.rs`):* Strips port and normalizes IPv6 brackets (`[::1]` -> `::1`). Rejects malformed Host headers (commas, spaces, control characters, userinfo injection `@`). Enforces exact and wildcard subdomain matching (`*.example.com`) while preventing lookalike domain spoofing (`evil-example.com`, `notexample.com`). Direct requests without verified peer IP strictly strip forwarded headers. Implements `resolve_client_ip` walking multi-hop `X-Forwarded-For` from right to left to locate the verified client IP at the trusted proxy boundary.
- **Consequences:** All four operational gaps are completely resolved and verified by 242 automated tests across 15 test suites with 0 failures, 0 Clippy warnings, and clean release compilation. Phase 11 remains strictly on hold.




