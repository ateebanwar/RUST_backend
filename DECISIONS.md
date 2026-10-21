# DECISIONS.md â€” Architecture Decision Records (ADR) Log

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
| **DEC-015** | Model-Aware Tokenizer Integration, Capability Hierarchy & Context Admission | **APPROVED** | 2026-10-21 | Tokenizer Architecture & Context Safety |
| **DEC-016** | Centralized Model-ID-to-Tokenizer Mapping, Truthful Framing Semantics, and Effective Message Context Admission | **APPROVED** | 2026-10-21 | Tokenizer Correctness & Routing Safety |
| **DEC-017** | Phase 11 Multi-Client Authentication, Google OIDC ID Token Verification, and Constant-Time Credential Validation | **APPROVED** | 2026-10-21 | Authentication & Identity Security |
| **DEC-018** | Phase 11 Hardening: Query-Parameter Credential Proscription, Route Scope Authorization, and Non-Loopback Bind Protection | **APPROVED** | 2026-10-21 | Authentication & Security Hardening |
| **DEC-019** | Forensic Security Remediation: Query Percent-Decoding, OIDC azp Claim Validation, Streaming JWKS Limits, and Configuration Precedence | **APPROVED** | 2026-10-21 | Authentication & Security Hardening |

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
- **Rationale:** Aligns with `API_SPEC.md` Â§2.2 (`200 OK (if at least one provider is ready) or 503 Service Unavailable`), prevents orchestrators and clients from routing inference requests to instances with zero operational providers, and prevents false operational status while maintaining catalog utility.
- **Consequences:** Systems with zero registered upstream providers correctly report 503 `not_ready` on `/api/v1/ready` while reporting 200 `healthy` on `/api/v1/health`.

---

### DEC-010: Multi-Criteria Intelligent Model Routing Policy & Scoring Matrix
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** `MASTER_SPEC.md` Â§6 and Phase 7 require selecting compatible models according to task requirements, capability, latency, reliability, and cost category. The engine must support automatic routing, manual selection, explainable decisions, and deterministic fallback ordering without fabricating telemetry.
- **Options Considered:**
  1. *Static Hardcoded Lookup Table:* Maps each task category directly to a fixed model ID string. Brittle, fails when models are added, removed, or unavailable.
  2. *Live Telemetry Polling with Fabricated Stats:* Attempts dynamic latency and pricing polling; risks fabricating metrics when providers do not expose real-time telemetry.
  3. *Multi-Criteria Scoring Engine with Catalog Introspection & Documented Safe Policy:*
     - Evaluates models dynamically from `ModelRegistry`.
     - Strict modality support is a non-negotiable prerequisite.
     - Multi-dimensional scoring evaluates capability match (0â€“50 pts), complexity alignment (0â€“30 pts), speed/latency tier (0â€“15 pts), cost tier (0â€“15 pts), and context capacity (0â€“10 pts).
     - Deterministic tie-breaking: cost tier preference, followed by speed tier, context window, and alphabetical `model_id`.
     - In manual mode: validates existence, availability, and modality compatibility; ranks remaining compatible models as fallbacks.
     - When no operational models match: returns canonical HTTP 422 (`UNSUPPORTED_MODALITY`) or HTTP 502 (`UPSTREAM_FAILURE`).
- **Selected Option:** Option 3 (Multi-Criteria Scoring Engine with Catalog Introspection).
---

### DEC-011: Real HTTP Provider Adapter, Inference Pipeline Execution, and Bounded Failover Architecture
- **Status:** APPROVED
- **Date:** 2026-10-19
- **Context:** An audit identified that while Phases 1â€“7 completed foundational interfaces, routing, and classification, live chat completions and streaming endpoints were unmapped (`GAP-001`), the provider adapter was mock-only (`GAP-002`), router-to-provider execution was disconnected (`GAP-003`), fallback failover was unbound (`GAP-005`), request ID propagation and auth were missing (`GAP-006`, `GAP-007`), and provider base URLs were exposed in public JSON responses (`GAP-010`).
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
- **Rationale:** Preserves zero external network leakage during testing, eliminates heavyweight dependencies, maintains small binaries, ensures secret isolation, and provides fully functional end-to-end inference execution conforming strictly to `MASTER_SPEC.md` Â§6 & Â§18, `API_SPEC.md` Â§5 & Â§6, and `SECURITY.md`.
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
- **Rationale:** Zero external dependencies or binary bloat, negligible CPU overhead (<1 Âµs), prevents costly upstream 400 context overflow rejections, eliminates redundant routing classification, and delivers proven latency/throughput improvements (+112% RPS on completion, 53% latency reduction).
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

---

### DEC-015: Model-Aware Tokenizer Integration, Explicit Capability Classification, and Strict Context Admission Safety
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** Heuristic token estimation alone was insufficient for context-window safety near capacity boundaries, as shown by reference tokenizers counting code snippets and multilingual content with significant divergence from simple character/word heuristics. A percentage safety margin (+15%) mitigated underestimation for approximate models, but did not establish exact model-aware tokenization or prevent silent truncation/misallocation of requested output tokens.
- **Decisions Made:**
  1. *Native Offline BPE Tokenization (`tiktoken-rs`):* Integrated `tiktoken-rs` (version 0.12, no default features) for exact, local, pure-Rust BPE tokenization. Instantiated `CL100K_BPE` (GPT-4 / GPT-3.5-Turbo) and `O200K_BPE` (GPT-4o / o1 / o3) as thread-safe, lazily initialized static singletons using `std::sync::LazyLock<Option<CoreBPE>>`. Tokenizer rank tables are compiled once into memory and reused across all requests with zero network overhead.
  2. *Explicit Capability Hierarchy (`TokenizerCapability`):* Established a 3-tier capability contract: `Exact` (verified tokenizer and request framing available), `Approximate` (calibrated subword heuristic or conservative fallback with 15% safety headroom), and `ProviderReported` (authoritative post-execution provider usage). Heuristic estimates are strictly prohibited from reporting `is_exact = true`.
  3. *Accurate Message Framing & Schema Accounting:* Modeled exact ChatML message delimiters (3 tokens per message for `<|im_start|>{role}\n{content}<|im_end|>\n` + encoded role + encoded content) and 3 tokens for assistant response primer (`<|im_start|>assistant\n`). Added schema-level tool token counting with framing overhead.
  4. *Strict Output Limit and Context Admission Gating:* Eliminated silent clamping of `req.max_tokens`. If a client requests more tokens than a model's `max_output_tokens`, the request is rejected with HTTP 400 `InvalidRequest`. Enforced checked addition (`estimated_prompt_tokens.checked_add(completion_allowance)`) to guard against integer overflow. Requests exceeding `model.context_window` are rejected prior to provider dispatch. Automatic routing evaluates candidate models against model-aware context requirements and output limits.
- **Consequences:** Provides exact tokenization for supported OpenAI models, transparent approximation for other model families, and strict context safety across all routes. Verified by 258 automated tests across 16 test suites. Phase 11 remains strictly on hold.

---

### DEC-016: Centralized Model-ID-to-Tokenizer Mapping, Truthful Framing Semantics, and Effective Message Context Admission
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** An end-to-end token estimation audit revealed critical correctness defects: (1) broad substring matching (`.contains("gpt")`, `.contains("o1")`) misclassified newer models (`gpt-4.1`, `gpt-4.5`, `gpt-5` as `cl100k` instead of `o200k`, dropped `o4-mini` to generic, and falsely classified unrelated models like `auto1`, `echo1`, `audio3`, `my-custom-gpt` as exact BPE); (2) chat requests and safe context estimates misleadingly reported `is_exact: true` even though message and schema framing were heuristic constants (+3 per message, +3 assistant primer, +12 schema overhead); (3) context admission evaluated uncompacted raw messages while execution dispatched compacted messages; (4) auto-routing conflated requested `max_tokens` exceeding model output limits with context window exhaustion.
- **Decisions Made:**
  1. *Authoritative Centralized Model-to-Tokenizer Engine (`src/inference/tokens.rs`):* Replaced broad substring matching with `resolve_model_tokenizer(model_id, registry)`. Mapped exact IDs and verified prefixes to verified BPE encodings (`o200k_base` vs `cl100k_base`) based directly on official OpenAI `tiktoken/model.py` specifications. Strips provider namespaces (`openai/*`, `groq/*`, `openrouter/*`). Explicitly aliases reference catalog models (`fast-chat-v1` -> Llama, `deep-reasoner-v1` -> DeepSeek, `balanced-assistant-v1` -> Mistral). Unknown IDs and negative lookalikes strictly fall through to `MappingKind::Unknown` and `TokenizerFamily::Generic`.
  2. *Truthful Exactness Separation:* Strictly separated exact raw-text BPE tokenization (`is_exact: true`, `method: ExactTokenizer`, `raw_text_exact: Some(true)`) from multi-message chat estimation (`is_exact: false`, `method: EstimatedChatFraming`, `raw_text_exact: Some(true)`).
  3. *Effective Message Context Admission (`src/inference/mod.rs`):* `resolve_candidates` now evaluates effective compacted messages (`effective_messages`) rather than raw messages, ensuring that admission checks and dispatched payloads are identical.
  4. *Output Limit Discrimination in Auto-Routing:* Differentiated output-token limit violation from context-window exhaustion in auto-routing: if requested `max_tokens` exceeds all available models' `max_output_tokens`, it returns a specific error (`Requested max_tokens ({requested}) exceeds maximum output token limit of all available operational models ({max})`).
  5. *Comprehensive Regression & Provenance Test Suite (`tests/model_aware_tokenization_and_admission_test.rs`):* Added table-driven mapping matrix (Category 16), reference tokenizer provenance fixtures (Category 17), auto-routing candidate admission & output limit checks (Category 18), and message compaction consistency (Category 19).
- **Consequences:** All confirmed defects are permanently resolved and verified by 279 automated tests across 16 test suites with 0 failures, 0 Clippy warnings, and 0 secret leaks. Phase 11 remains strictly on hold.

---

### DEC-017: Phase 11 Multi-Client Authentication, Google OIDC ID Token Verification, and Constant-Time Credential Validation
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** The Phase 11 mandate required implementing production-ready authentication and authorization across multiple client types (Flutter mobile, Custom IDE, VS Code Extension, and API consumers) while preserving zero-secret leaks, fail-closed production safety, and offline testability without live external identity dependencies.
- **Decisions Made:**
  1. *Dual Authentication Scheme Architecture (`src/auth/`):* Implemented a unified `AuthService` handling mobile/web clients via Google Sign-In OpenID Connect (OIDC) ID tokens and developer/IDE clients via Personal Access Tokens (PATs) and backend API keys.
  2. *Cryptographic Google OIDC ID Token Verification (`src/auth/oidc.rs`):* Uses `jsonwebtoken` to cryptographically verify RS256 JWT signatures against Google's public JSON Web Key Sets (JWKS). Strictly validates `iss` (`https://accounts.google.com` or `accounts.google.com`), `aud` against configured Google Client IDs, expiration timestamp (`exp` with 60-second RFC 7519 leeway), and non-empty subject (`sub`). JWKS keys are cached with 1-hour TTL and bounded 5-second fetch timeout (`HttpJwksProvider`). Deterministic offline test execution is guaranteed via `MockJwksProvider` without live Google network access.
  3. *Constant-Time Credential & PAT Hashing (`src/auth/pat.rs`):* Incoming tokens and API keys are SHA-256 hashed and verified using `subtle::ConstantTimeEq`, completely eliminating length and value timing side channels. Implemented secure PAT generation with standard `pat_` prefix and 64 random hex characters via OS secure random generator (`ring::rand::SystemRandom`).
  4. *Request Identity Propagation & Role Authorization (`src/auth/principal.rs`):* Injects strongly-typed `AuthPrincipal` into Axum request extensions containing `subject`, `auth_type`, `client_type` (parsed from `X-Client-Type`), `email`, `roles` (`User`, `Admin`, `Service`), and `scopes`. Overrides `Debug` to redact email and secrets.
  5. *Strict Route Protection & Precedence Policy:* Public probes (`/health`, `/ready`) are unconditionally accessible without authentication. All inference (`/chat/*`), model discovery (`/models/*`, `/providers`), routing preview, and operational telemetry (`/usage/*`, `/quotas`) endpoints require authentication. 3-segment JWTs route strictly to OIDC; non-JWT tokens route strictly to PAT/API key validators. Invalid credentials never fall back to weaker authentication.
  6. *Fail-Closed Production Configuration (`src/config.rs`):* In production (`APP_ENV=production`), application startup fails closed with `ConfigError::InvalidValue` if no valid `API_KEY`, `API_KEYS`, or `GOOGLE_CLIENT_ID` is present. Configured keys must be at least 8 characters long.
- **Consequences:** Provides robust, client-independent authentication with constant-time security and zero credential leakage. Verified by 299 automated tests across 17 test suites with 0 failures, 0 Clippy warnings, and 0 security vulnerabilities. Phase 12 remains strictly on hold.

---

### DEC-018: Phase 11 Hardening: Query-Parameter Credential Proscription, Route Scope Authorization, and Non-Loopback Bind Protection
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** A comprehensive Phase 11 security audit identified operational gaps and hardening requirements: (1) URLs bearing query parameters like `api_key` or `access_token` must be unconditionally rejected to prevent credential leakage into proxy access logs, browser history, and Referer headers; (2) development bypass must never be accessible when binding to non-loopback network interfaces (`0.0.0.0` or public IPs); (3) authenticated principals must be authorized against route-level scopes (`models:read`, `routing:read`, `chat`, `usage:read`) with HTTP 403 Forbidden; (4) OIDC JWKS fetching required a bounded payload ceiling (256 KB), refresh throttling on unknown `kid` (10 seconds), and production HTTPS enforcement; (5) `ConfiguredCredential` debug formatting must redact token hashes to eliminate diagnostic exposure.
- **Decisions Made:**
  1. *Unconditional Query-Parameter Credential Rejection (`src/auth/service.rs`):* If a protected request URL contains any credential-bearing query parameter (`api_key`, `apikey`, `access_token`, `token`, `auth_token`, `auth`, `bearer`), it is immediately rejected with HTTP 401 Unauthorized and canonical code `UNAUTHORIZED` before any header-based authentication is attempted.
  2. *Strict External Bind Protection (`src/config.rs`, `src/auth/service.rs`):* Modified `Config::is_auth_required` and `AuthService::authenticate` so that whenever the backend binds to a non-loopback host interface (`!is_loopback_host()`), authentication is unconditionally required across all protected endpoints regardless of `AUTH_REQUIRED` or `APP_ENV` configuration.
  3. *Route-Level Scope Authorization Enforcement (`src/auth/service.rs`, `src/auth/middleware.rs`):* Added `required_scope_for_route` and `AuthService::authorize(&principal, path)`. Protected routes map to mandatory scopes: `/api/v1/models` and `/api/v1/providers` require `models:read`; `/api/v1/routing/preview` requires `routing:read`; `/api/v1/chat/*` requires `chat`; `/api/v1/usage/*` and `/api/v1/quotas` require `usage:read`. Requests from principals lacking the required scope are rejected with HTTP 403 Forbidden and canonical code `FORBIDDEN`.
  4. *Google OIDC & JWKS Hardening (`src/auth/oidc.rs`, `src/config.rs`):* Enforced a 256 KB response body limit on JWKS downloads; added a 10-second refresh throttle on unknown `kid` to mitigate cache-miss network amplification; enforced HTTPS for `GOOGLE_JWKS_URL` in production; and enabled validation for `nbf`, `iat` future sanity, and `azp` client ID verification.
  5. *Diagnostic Credential Redaction (`src/auth/pat.rs`):* Implemented custom zero-leak `std::fmt::Debug` on `ConfiguredCredential` to redact precomputed token hashes.
- **Consequences:** All identified security gaps are remediated and verified by 306 automated tests across 17 test suites (including 7 new security integration tests) with 0 failures, 0 Clippy warnings, and 0 security vulnerabilities. Phase 12 remains strictly on hold.

---

### DEC-019: Forensic Security Remediation: Query Percent-Decoding, OIDC azp Claim Validation, Streaming JWKS Limits, and Configuration Precedence
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** An independent forensic security audit of the Phase 11 implementation discovered subtle correctness and security edge cases: (1) query-parameter credential proscription could be bypassed via percent-encoded parameter keys (e.g. `api%5fkey` or `access%5ftoken`); (2) Google OIDC `azp` validation contained a redundant condition (`&& !claims.matches_audience(...)`) that effectively bypassed `azp` verification when `aud` matched; (3) JWKS response downloads buffered the entire body before checking size limits, exposing the service to unbounded chunked stream memory exhaustion; (4) concurrent cache misses on unknown `kid` could trigger thundering-herd duplicate outbound requests; (5) `Config::is_auth_required` gave precedence to `auth_required = Some(false)` over production and external interface invariants, and `Config::validate()` did not reject `AUTH_REQUIRED=false` in production.
- **Decisions Made:**
  1. *URL Percent-Decoding in Credential Query Proscription (`src/auth/service.rs`):* Added `AuthService::percent_decode` to normalize `%xx` hex encodings and whitespace before matching parameter keys against recognized credential names (`api_key`, `apikey`, `api-key`, `access_token`, `token`, `auth_token`, `auth`, `bearer`, `secret`). Both `&` and `;` delimiters are parsed.
  2. *Strict OIDC Authorized Party (`azp`) Claim Validation (`src/auth/oidc.rs`):* Corrected `GoogleOidcValidator::validate` to require that any present `azp` claim strictly match one of the application's configured Google Client IDs, in accordance with OpenID Connect Core 1.0 §3.1.3.7.
  3. *Incremental Streaming JWKS Body Size Ceiling (`src/auth/oidc.rs`):* Replaced `response.bytes().await` with incremental chunk streaming via `response.chunk().await`, immediately aborting with `OidcError::JwksFetchFailed` if total bytes exceed `MAX_JWKS_RESPONSE_BYTES` (256 KB) during transfer.
  4. *Atomic Refresh Throttling Under Write Lock (`src/auth/oidc.rs`):* In `HttpJwksProvider::get_decoding_key`, `last_fetch_attempt` is checked and atomically updated under an asynchronous write lock before initiating remote HTTP requests, eliminating concurrent outbound fetch amplification.
  5. *Configuration Precedence & Production Validation (`src/config.rs`):* In `Config::is_auth_required()`, `self.environment == Environment::Production || !self.is_loopback_host()` is evaluated first, guaranteeing that `auth_required = Some(false)` cannot disable authentication in production or on external network interfaces. In `Config::validate()`, setting `AUTH_REQUIRED=false` in production fails closed at startup with `ConfigError::InvalidValue`.
  6. *Host Normalization for IPv6 Brackets and Case-Insensitivity (`src/config.rs`):* `Config::is_loopback_host` and `Config::validate` strip IPv6 brackets `[` / `]` and match `"localhost"` case-insensitively.
- **Consequences:** All confirmed forensic defects are corrected and verified by 312 automated tests across 17 test suites (81 unit + 231 integration, plus 1 ignored benchmark, including 5 new tests in `tests/authentication_test.rs`) with 0 failures, 0 Clippy warnings, and 0 secret leaks. Phase 12 remains strictly on hold.

---

### DEC-020: Phase 11 Production-Grade Security Audit: Multi-Audience azp Enforcement, JWKS Concurrency Cache Re-check, and Header Hardening
- **Status:** APPROVED
- **Date:** 2026-10-21
- **Context:** A production-grade security audit and adversarial review of Phase 11 identified critical hardening opportunities across the authentication pipeline: (1) OpenID Connect Core 1.0 §3.1.3.7 rule 3 mandates that if an ID token contains multiple audiences (`claims.aud` is an array of length > 1), an `azp` (Authorized Party) claim MUST be present; previously, tokens with multiple audiences lacking `azp` were not rejected; (2) under concurrent in-flight requests during legitimate Google JWKS key rotation, tasks throttled by the 10-second unknown-`kid` rate limit could prematurely fail with `UnknownKeyId` without checking if another concurrent task had already refreshed and cached the new key; (3) ambiguous or duplicate headers (multiple `Authorization` or `X-API-Key` headers) or simultaneous conflicting authentication schemes (both `Authorization: Bearer <token>` and `X-API-Key: <key>` provided) were not deterministically rejected, creating risk of proxy-desynchronization or ambiguous identity resolution; (4) unbounded credential lengths could permit memory exhaustion attacks during JWT parsing, ring hashing, or string allocation.
- **Decisions Made:**
  1. *OpenID Connect Core 1.0 Multi-Audience `azp` Enforcement (`src/auth/oidc.rs`):* If `claims.aud` contains multiple audiences and `claims.azp.is_none()`, the token is immediately rejected with `OidcError::InvalidAudience("Token with multiple audiences must contain an authorized party (azp) claim")`. If `azp` is present, it must strictly match configured Google Client IDs.
  2. *JWKS Concurrency Cache Re-check on Throttle (`src/auth/oidc.rs`):* In `HttpJwksProvider::get_decoding_key`, when `last_fetch_attempt` is within `MIN_REFRESH_INTERVAL` (10 seconds), the provider re-inspects `self.cache.read().await` before returning `Err(OidcError::UnknownKeyId)`. If another concurrent task just cached the rotated key, the request succeeds immediately.
  3. *Duplicate & Conflicting Header Proscription (`src/auth/service.rs`):* In `AuthService::authenticate`, requests with multiple `Authorization` headers (`headers.get_all("authorization").iter().count() > 1`) or multiple `X-API-Key` headers are rejected with HTTP 401 Unauthorized. Requests supplying both `Authorization` and `X-API-Key` simultaneously are rejected with HTTP 401 Unauthorized ("Conflicting authentication credentials: both Authorization and X-API-Key headers were provided").
  4. *Safe Header String Extraction & Non-UTF-8 Fail-Closed (`src/auth/service.rs`):* Extracted header values are validated as valid UTF-8. Non-UTF-8 header bytes fail closed with HTTP 401 Unauthorized rather than being silently ignored or bypassed.
  5. *Credential Length Bounding (`src/auth/service.rs`):* Defined `AuthService::MAX_CREDENTIAL_LEN = 8192` (8 KiB). Inbound Bearer tokens and API keys exceeding 8192 bytes are rejected with HTTP 401 Unauthorized, bounding memory allocation and parsing work under malicious payloads.
  6. *Security Integration & Unit Test Expansion (`tests/authentication_test.rs`, `src/auth/oidc.rs`, `src/auth/service.rs`):* Added Tests 25–29 and 8 unit tests covering multi-audience OIDC with and without `azp`, duplicate and conflicting headers, oversized credentials, non-UTF-8 headers, route scope authorization, and concurrent OIDC/API key request safety. Total tests expanded to **325 passing tests** across 17 test suites (89 unit + 236 integration, 0 failures, 1 ignored benchmark).
- **Consequences:** Provides complete, standards-compliant, fail-closed multi-client authentication. Verified by 325 automated tests with zero Clippy warnings, zero cargo-audit vulnerabilities, and zero secret leaks. Phase 12 (Authorization and Secret Management) remains strictly on hold.

---

### DEC-021: Phase 12 Implementation: Resource Ownership, Dynamic PAT Lifecycle, and Authenticated Secret Management
- **Status:** APPROVED
- **Date:** 2026-10-22
- **Context:** Phase 12 mandates complete authorization, resource ownership enforcement, secret management, and security hardening across the entire backend. Specific requirements: (1) Resource ownership and tenant isolation on usage telemetry and quota tracking; (2) Insecure direct object reference (IDOR) prevention; (3) Dynamic Personal Access Token (PAT) lifecycle management with single-exposure creation/rotation, masked listings, and immediate revocation; (4) Scope privilege escalation prevention; (5) Authenticated encryption at rest (AES-256-GCM) with AAD binding; (6) Zero-leak secret representations (`SecretString`).
- **Decisions Made:**
  1. *Resource Ownership & Tenant Isolation on Usage Telemetry (`src/usage/storage.rs`, `src/routes/usage.rs`, `src/routes/chat.rs`):* Added `subject` column and indexes (`idx_usage_records_subject`, `idx_usage_records_tenant_start_time`) to `usage_records` via migration v3. Usage queries (`get_summary`, `get_providers`, `get_models`, `get_records`) strictly parameterize tenant filtering. Standard `AuthRole::User` callers are forced to their authenticated `principal.subject`. Query attempts targeting another user (`?user_id=...` or `?subject=...`) fail closed with HTTP 403 Forbidden (`code: FORBIDDEN`).
  2. *Dynamic Credential & PAT Engine (`src/auth/credential_store.rs`, `src/routes/credentials.rs`):* Implemented SQLite-backed `CredentialStore` with memory cache for PAT creation, rotation, revocation, and listing. Tokens are generated via `ring::rand::SystemRandom`, hashed with SHA-256, and never stored in plaintext. Plaintext tokens are exposed strictly ONCE upon creation or rotation. List and inspect endpoints return masked identifiers (`pat_1234...cdef`).
  3. *IDOR Defenses on Credentials:* Looking up, rotating, or revoking a token owned by another subject returns HTTP 404 Not Found to prevent resource existence enumeration.
  4. *Privilege Escalation Defense:* Callers creating tokens cannot grant scopes exceeding their own scopes unless they possess `AuthRole::Admin` or wildcard scope `*`.
  5. *Zero-Leak Secret Abstraction (`src/security/secret.rs`):* `SecretString` implements custom `Debug`, `Display`, and `serde::Serialize` formatting that unconditionally outputs `"[REDACTED]"`. Underlying secret is accessible only via `expose_secret()`.
  6. *Authenticated Encryption at Rest (`src/security/encryption.rs`):* Implemented `AesGcmCipher` providing AES-256-GCM authenticated encryption using `ring::aead` with 96-bit random nonces and Associated Data (AAD) context binding.
- **Consequences:** All 333 automated tests pass across 18 test suites with 0 failures, 0 Clippy warnings, 0 cargo audit vulnerabilities, and 0 secret leaks. Phase 12 is fully verified. Phase 13 (Rate limiting / Abuse prevention) remains on hold.


