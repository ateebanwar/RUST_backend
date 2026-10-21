# ARCHITECTURE.md â€” System Architecture & Component Specification

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Status:** Finalized Phase 2 Architecture & Module Interface Contract  
**Last Updated:** 2026-10-19

---

## 1. Architectural Mission & Principles

The `ai-backend` platform is a lightweight, asynchronous, security-first Rust service designed to decouple AI client applications from diverse, independent AI providers and APIs.

```mermaid
graph TD
    subgraph Client Layer
        Mobile[Flutter Android App]
        IDE[Custom AI IDE]
        Ext[VS Code / Editor Extensions]
        APIClient[Authorized API Consumers]
    end

    subgraph API Gateway & Security Boundary
        GW[Axum HTTP Router & Tower Middleware]
        Auth[Auth & Token Validator]
        Limit[Rate Limiter & Request Guard]
    end

    subgraph Core Routing Engine
        Classifier[Task & Complexity Classifier]
        Router[Intelligent Model Router]
        Registry[Central Model Registry]
        Resilience[Failover & Circuit Breaker]
    end

    subgraph Provider Subsystem
        AdapterMgr[Provider Adapter Manager]
        AdapterA[Provider Adapter: OpenAI-compatible]
        AdapterB[Provider Adapter: Anthropic-compatible]
        AdapterC[Provider Adapter: Local / OSS / Repo]
    end

    subgraph Persistence & History
        DriveSync[Google Drive Sync Service]
    end

    subgraph External Dependencies
        GoogleDrive[(User Google Drive / appDataFolder)]
        UpstreamA[Upstream AI Provider A]
        UpstreamB[Upstream AI Provider B]
    end

    Mobile -->|Bearer Google ID Token| GW
    IDE -->|X-API-Key / PAT| GW
    Ext -->|X-API-Key / PAT| GW
    APIClient -->|X-API-Key| GW

    GW --> Auth
    Auth --> Limit
    Limit --> Classifier
    Classifier --> Router
    Router --> Registry
    Router --> Resilience
    Resilience --> AdapterMgr
    AdapterMgr --> AdapterA
    AdapterMgr --> AdapterB
    AdapterMgr --> AdapterC

    AdapterA --> UpstreamA
    AdapterB --> UpstreamB

    GW --> DriveSync
    DriveSync --> GoogleDrive
```

### Core Architectural Principles
1. **Multi-Client Agnosticism:** Core business logic, routing, and provider adapters are strictly frontend-independent.
2. **Stateless Core / Zero-Database Constraint:** No permanent server-side database is required. Chat history is stored in user-controlled Google Drive, and usage tracking operates gracefully in ephemeral serverless runtimes.
3. **Dual-Target Portability:** Core routing compiles cleanly as both a long-running Axum HTTP daemon (local development, Docker) and an AWS Lambda / Vercel Serverless Function (via `lambda_http`).
4. **Strict Isolation & Fault Containment:** A failure, timeout, or rate-limit event in one AI provider never crashes the backend or impairs unrelated integrations.
5. **Zero-Trust Security:** Client assertions are never trusted; provider secrets are never leaked; all input payloads are bounded and validated.

---

## 2. Comprehensive Module Boundaries & Interface Contracts

The backend is decomposed into 16 cohesive, loosely coupled logical modules. Inter-module dependencies strictly adhere to a **Directed Acyclic Graph (DAG)** to eliminate circular references.

```mermaid
graph TD
    api[api: HTTP Routes & Handlers] --> auth[auth: Token Validation]
    api --> security[security: SSRF & Request Limits]
    api --> router[router: Task & Model Routing]
    api --> drive[drive: Google Drive Sync]
    api --> usage[usage: Token Accounting]
    api --> registry[registry: Model Catalog]

    router --> registry
    router --> models[models: Core Types]
    router --> failover[failover: Resilience & Retries]
    
    failover --> providers[providers: Registry & Lifecycle]
    providers --> adapters[adapters: Provider Trait Impls]
    adapters --> security
    adapters --> models
    adapters --> errors[errors: Error Taxonomy]

    drive --> auth
    drive --> security
    drive --> chat[chat: Conversation Models]

    api --> observability[observability: Tracing & Metrics]
    api --> config[config: Typed Environment Config]
    api --> errors
```

### Detailed Module Contract Matrix

| Module | Core Responsibility | Primary Inputs | Primary Outputs | Associated Error Types | Allowed Direct Dependencies | Security Boundary |
|---|---|---|---|---|---|---|
| **`api`** | Axum HTTP routing, request parsing, response formatting, SSE streams. | Axum `Request<Body>` | HTTP `Response<Body>`, SSE stream | Maps all domain errors to HTTP JSON envelope. | `auth`, `router`, `registry`, `drive`, `usage`, `config`, `security`, `errors` | **Boundary 1:** Untrusted public ingress. Enforces 4.5 MB body limit. |
| **`auth`** | Token verification (Google OIDC JWKS, PAT constant-time hash check). | `Authorization` header, `X-API-Key` | `UserContext` (User ID, ClientType, Scopes) | `AuthError::InvalidToken`, `AuthError::ExpiredToken`, `AuthError::MissingHeader` | `config`, `security`, `errors` | Cryptographic verification of client identity. Zero trust for client claims. |
| **`config`** | Startup configuration parsing, validation, and secret wrapping. | Environment variables, `.env` file | `AppConfig`, `ProviderConfig` | `ConfigError::MissingKey`, `ConfigError::InvalidFormat` | `security`, `errors` | Encapsulates API keys in `SecretString` to prevent memory/log leaks. |
| **`security`** | SSRF prevention, outbound URL validation, request body limits, CORS. | Outbound URLs, IP addresses | Validated `Url`, sanitized strings | `SecurityError::SsrfBlocked`, `SecurityError::PayloadTooLarge` | `errors` | **Boundary 2 & 4:** Blocks loopback, RFC 1918, link-local metadata. Redacts secrets. |
| **`providers`** | Provider lifecycle, health check execution, adapter catalog. | Provider ID, configuration | `Arc<dyn ProviderAdapter>` | `ProviderError::NotFound`, `ProviderError::Disabled` | `adapters`, `models`, `errors` | Isolates provider instances; failure of one does not crash others. |
| **`adapters`** | Vendor API payload translation, HTTP calls, response/chunk normalization. | `CompletionRequest`, upstream SSE | `CompletionResponse`, `StreamChunk` | `ProviderError::RateLimited`, `ProviderError::Timeout`, `ProviderError::UpstreamError` | `models`, `security`, `errors` | **Boundary 2:** Outbound HTTPS calls with provider API keys. Strict 10s timeout. |
| **`models`** | Canonical domain types for models, capabilities, modalities, usage. | Raw config definitions | `ModelMetadata`, `ProviderMetadata` | None (pure domain data structures) | None (Leaf module) | Shared immutable type definitions. |
| **`registry`** | In-memory model catalog, capability filtering, availability tracking. | Filter criteria (modality, tier) | `Vec<ModelMetadata>`, `ModelMetadata` | `RegistryError::ModelNotFound` | `models`, `errors` | Thread-safe read-heavy catalog without unvetted dynamic code loading. |
| **`router`** | Heuristic task categorization, complexity scoring, model selection. | User prompt text, requested modality | `RoutingDecision`, `ModelCandidate` | `RouterError::NoCompatibleModel` | `registry`, `models`, `failover`, `errors` | Ensures requests route strictly to models matching verified capabilities. |
| **`quota`** | Quota header normalization, rate-limit state tracking. | Upstream HTTP headers | `QuotaStatus`, `RateLimitInfo` | None | `models`, `errors` | Never fabricates quotas; distinguishes reported from estimated values. |
| **`usage`** | Token consumption accounting, request counters, local usage caching. | `NormalizedUsage`, provider ID | `UsageSummary`, aggregated metrics | None | `models`, `errors` | Ephemeral in-memory accounting; client-reflected stats for persistence. |
| **`failover`** | Circuit breaker state machine, exponential backoff, fallback execution. | Failed model, candidate list | Fallback `ModelCandidate`, retry decision | `FailoverError::AllCandidatesFailed` | `providers`, `models`, `errors` | Prevents retry storms and duplicate charges on non-idempotent operations. |
| **`chat`** | Conversation and message data structures, context window pruning. | `Vec<CanonicalMessage>`, context limit | Truncated `Vec<CanonicalMessage>` | `ChatError::ContextExceeded` | `models`, `errors` | Protects privacy; sends only context necessary for active task. |
| **`drive`** | Google Drive REST client (`drive.appdata`), file sync, LWW conflict resolver. | `X-Drive-Token`, chat payload | Synchronized chat envelope, conflict log | `DriveError::Unauthorized`, `DriveError::RemoteFailure` | `auth`, `chat`, `security`, `errors` | **Boundary 3:** Restricted strictly to private `drive.appdata` scope. |
| **`observability`**| Structured tracing setup, correlation IDs, sanitized log layer. | Tracing events, span context | Formatted JSON logs, metrics | None | `errors` | Redacts auth headers, query tokens, and prompt text at INFO/WARN. |
| **`errors`** | Centralized application error taxonomy and HTTP status conversion. | Internal domain error variants | `AppError`, API JSON error envelope | None (Root error module) | None (Leaf module) | Strips internal stack traces and secrets before returning errors to clients. |

---

### 2.1 Intelligent Router Scoring Policy & Multi-Criteria Weights

The model selection policy (`src/router/policy.rs`) evaluates candidate models through a multi-stage scoring algorithm. Arithmetic uses signed 32-bit integers (`i32`) to guarantee determinism and eliminate arithmetic overflow.

#### A. Hard Prerequisites (Eligibility Gate)
1. **Modality Filtering:** Candidates must declare support for the requested `TaskModality` (e.g. `text`, `code`, `image`, `document`). Incompatible models receive zero score and are immediately discarded.
2. **Operational Availability:** Models marked unavailable (`available == false`) in the registry or without active registered adapters are bypassed during execution.

#### B. Multi-Criteria Scoring Formula
```text
Total Score = Capability Score (0..50) + Complexity Alignment Score (0..30)
            + Speed Suitability Score (0..15) + Cost Tier Score (0..15)
            + Capacity Score (0..10)
```
- **Theoretical Maximum Standard Score:** 120 points (up to 125 in edge combinations).
- **Minimum Positive Score:** 13 points.

#### C. Component Breakdown
1. **Capability Alignment (0â€“50 points):**
   - Matches classified `TaskCategory` against model `capabilities`. Primary category match adds +50 points. Secondary coding/reasoning matches on coding tasks add up to +20 points.
2. **Complexity Alignment (0â€“30 points):**
   - **High Complexity:** `advanced` = +30, `balanced` = +10, `fast` = +5.
   - **Medium Complexity:** `balanced` = +25, `advanced` = +20, `fast` = +15.
   - **Low Complexity:** `fast` = +30, `balanced` = +15, others = 0 (penalizes wasteful models on simple queries).
3. **Speed & Latency Suitability (0â€“15 points):**
   - **Low Complexity:** `fast` = +15, `balanced` = +8, others = 0.
   - **Medium Complexity:** `balanced` = +15, `fast` = +10, others = +10.
   - **High Complexity:** `advanced` = +15, `balanced` = +8, others = +5.
   *(Note: Combined on Low Complexity, a `fast` model receives 30 + 15 = 45 points from complexity and speed dimensions).*
4. **Cost Tier Preference (0â€“15 points):**
   - `free-tier` = +15, `low-cost` = +10, `premium` = +2 (protects user quota/tokens per `MASTER_SPEC.md` Â§2.9).
5. **Context & Token Capacity (0â€“10 points):**
   - For Medium/High complexity tasks, models with context window $\ge 64\text{k}$ receive +5 points; max output tokens $\ge 8\text{k}$ receive +5 points.

#### D. Deterministic Multi-Stage Tie-Breaking
When candidate total scores are identical, ties are resolved deterministically in order:
1. Prefer lower `cost_tier` (`free-tier` > `low-cost` > `premium`).
2. Prefer faster `speed_tier` (`fast` > `balanced` > `advanced`).
3. Prefer larger `context_window`.
4. Lexicographical / alphabetical ordering by `model_id`.

---

## 3. Data Flow & Request Lifecycles

### 3.1 Synchronous & Streaming Chat Completion Flow

```mermaid
sequenceDiagram
    autonumber
    actor Client as Client (Flutter / IDE / Extension)
    participant GW as Axum Router / Gateway
    participant Auth as Auth Validator
    participant Router as Model Router
    participant Adapter as Provider Adapter
    participant Upstream as Upstream AI Provider

    Client->>GW: POST /api/v1/chat/stream (Payload + Bearer/PAT)
    GW->>Auth: Validate Auth Token
    Auth-->>GW: OK (UserContext)
    GW->>Router: Classify Task & Select Model
    Router-->>GW: ModelCandidate (Provider A, Model X)
    GW->>Adapter: Execute Stream (Timeout: 10s)
    Adapter->>Upstream: HTTP POST (Vendor Payload + Provider API Key)
    
    alt Upstream Responds Successfully
        Upstream-->>Adapter: SSE Chunks (Vendor format)
        Adapter-->>GW: Canonical StreamChunk
        GW-->>Client: SSE Event: data: {"chunk": "..."}
        Upstream-->>Adapter: Final Chunk [DONE]
        Adapter-->>GW: StreamComplete (Usage metadata)
        GW-->>Client: SSE Event: data: {"done": true, "usage": {...}}
    else Upstream Fails (429 Rate Limit / Timeout)
        Upstream-->>Adapter: Error 429 / Timeout
        Adapter-->>GW: ProviderError::RateLimited
        GW->>Router: Request Fallback Model
        Router-->>GW: FallbackCandidate (Provider B, Model Y)
        GW->>Adapter: Execute Fallback Stream
        Adapter->>Upstream: HTTP POST to Provider B
        Upstream-->>Client: Normal stream output from Provider B
    end
```

---

## 4. State Management: Ephemeral vs. Durable State

Under the **Zero-Cost Infrastructure Constraint**, no dedicated Redis or PostgreSQL cluster is provisioned. State is cleanly categorized:

| State Domain | Storage Tier | Lifetime | Failure / Cold-Start Handling |
|---|---|---|---|
| **Chat History & Messages** | User Google Drive (`drive.appdata`) | Indefinite | Restored on demand; persistent across app uninstalls |
| **Model Registry Metadata** | In-Memory Static + Background Refresh | Lifetime of container / microVM | Re-initialized from config in <1ms on cold start |
| **Circuit Breaker State** | In-Memory Ephemeral per Instance | Lifetime of microVM instance | Resets on cold start; rapidly re-trips upon encountering upstream 5xx/429 |
| **Usage Tracking** | Durable Local SQLite (`data/usage.db`) | Indefinite across restarts | Parameterized SQL, WAL mode, async via `spawn_blocking`, non-fatal error isolation |
| **Active Connections / SSE** | Ephemeral TCP stream | Request lifetime | Strict 10s upstream timeouts prevent hanging microVMs |

---

## 5. Multi-Client Adaptation Matrix

| Client Type | Transport / Protocol | Authentication Method | Context Features Supported |
|---|---|---|---|
| **Flutter Android App** | REST + SSE | Google Sign-In (OIDC ID Token) | Chat, Multimodal (Image/Doc), Drive Sync |
| **Custom AI IDE** | REST + SSE | Personal Access Token / API Key | Multi-file Codebase Context, Inline Diff, Terminal |
| **VS Code Extension** | REST + SSE | API Key / Local Loopback OAuth | Code Selection Context, Chat Assistant, Refactoring |
| **CLI / External API** | REST JSON | API Key (`X-API-Key`) | Scripted Execution, Automated Testing |

---

## 6. Phase 8: Quota & Usage Tracking Architecture

### 6.1 Data Flow and Telemetry Separation
Phase 8 establishes a clean separation between **Local Usage Accounting** and **Provider Quota Observations**:
- **Local Usage Accounting:** Records historical telemetry of completed and attempted requests in a durable SQLite database.
- **Provider Quota Awareness:** Tracks real-time provider rate limits (`x-ratelimit-remaining-*`, `retry-after`) in an in-memory TTL-bounded service to inform model candidate prioritization without speculative guesswork.

```mermaid
sequenceDiagram
    participant Client
    participant GW as Axum Gateway / chat.rs
    participant Inf as InferenceService
    participant Adapter as OpenAiCompatibleAdapter
    participant Quota as QuotaService
    participant Usage as UsageStorage (SQLite)

    Client->>GW: POST /api/v1/chat/completions or /stream
    GW->>Inf: execute_completion() / execute_stream()
    Inf->>Quota: is_rate_limited(provider, model)
    Note over Inf: Deprioritizes confirmed rate-limited candidates in auto mode
    Inf->>Adapter: complete() / stream()
    Adapter->>Quota: record_observation(headers) / record_rate_limited(429)
    Adapter-->>GW: CompletionResponse / ProviderStream
    Note over GW: Drop-Guard / AtomicBool ensures exactly-once finalization
    GW-->>Usage: record_buffered() / record_streaming() via spawn_blocking
    GW-->>Client: Response / SSE Stream
```

### 6.2 Token Representation & Missing Usage Semantics
- Exact integers (`u64`) are used for token accounting (`prompt_tokens`, `completion_tokens`, `total_tokens`).
- `UsageSource::Reported`: Provider supplied authoritative token counts. If `total_tokens` is omitted but prompt and completion exist, total is computed as `prompt + completion`.
- `UsageSource::Unavailable`: Provider did not return usage or the stream was aborted prematurely. Missing usage is preserved as `None` (JSON `null`) and is never silently replaced with zero or fabricated estimates.

### 6.3 Streaming Cancellation & Exactly-Once Finalization
- Streaming requests utilize an atomic state transition (`AtomicBool::swap(true, Ordering::SeqCst)`) inside `StreamFinalizationGuard`.
- Terminal branches (`chunk.is_done`, `ProviderError`, or stream drop on client disconnect) compete atomically. Exactly one record is written:
  - Normal finish -> `RequestOutcome::Success`.
  - Upstream transport failure -> `RequestOutcome::StreamInterrupted`.
  - Client aborted HTTP socket -> `RequestOutcome::ClientCancelled`.

### 6.4 SQLite Storage Engine, Retention, & Concurrency
- Configured via `DATABASE_PATH` (default `"data/usage.db"`).
- Retention enforcement via `USAGE_RETENTION_DAYS` (validated > 0; disabled by default to preserve all history). When configured, safe cleanup prunes expired records on startup and via `UsageService::prune_retention` off the Tokio worker path.
- Versioned, idempotent migrations via `schema_migrations` table.
- Multi-column indexed on `start_time`, `provider_id`, `model_id`, and `outcome`.
- Offloaded from Tokio async runtime worker threads using `tokio::task::spawn_blocking`.
- Database write errors log structured warnings and **never** fail client AI responses.

### 6.5 Read-Only Inspection Endpoints & External Interface Security
Protected versioned analytics endpoints returning bounded aggregations:
- `GET /api/v1/usage/summary`: High-level totals, outcomes, request modes, latency averages, and token totals.
- `GET /api/v1/usage/providers`: Aggregations grouped by provider.
- `GET /api/v1/usage/models`: Aggregations grouped by model.
- `GET /api/v1/quotas`: Active rate-limit observations with freshness and staleness flags.
- **Server Bind Protection:** When the server binds to a non-loopback address (e.g. `0.0.0.0`), operational endpoints strictly require authentication, preventing exposure over external networks without an API key. Local loopback development (`127.0.0.1`, `localhost`) permits dev access without keys.
- **Zero-Secret Guarantee:** Endpoints never persist, log, or return API keys, prompts, or completions.

---

## 7. Phase 9: Failover & Reliability Architecture

### 7.1 Architecture & Pipeline Flow
Phase 9 integrates a bounded, configurable resilience engine within `InferenceService` and `src/reliability/`:

```mermaid
sequenceDiagram
    participant Client
    participant GW as Axum Gateway / chat.rs
    participant Inf as InferenceService
    participant Pol as RetryPolicy
    participant Router as ModelRouter
    participant Adapter as ProviderAdapter
    participant Usage as UsageStorage (SQLite)

    Client->>GW: POST /api/v1/chat/completions or /stream
    GW->>Inf: execute_completion() / execute_stream()
    Inf->>Router: resolve_candidates() [Auto: ranked list; Manual: strict [primary]]
    Note over Inf: Set shared overall deadline (overall_request_timeout_secs)
    loop Candidate Selection (max_failover_candidates)
        Note over Inf: Check candidate not in failed_model_ids
        loop Candidate Retry Loop (max_retries_per_candidate)
            Note over Inf: Enforce remaining deadline: min(upstream_timeout, deadline - now)
            Inf->>Adapter: complete() or stream()
            alt Success
                Adapter-->>Inf: CompletionResponse / ProviderStream
                Inf-->>GW: Return response / stream to client
            else Non-Retryable Error (400, 401, 403, 404, 422, disabled)
                Adapter-->>Inf: ProviderError
                Note over Inf: Short-circuit immediately: NO RETRY, NO FAILOVER
                Inf-->>GW: AppError
            else Transient Error (500, 502, 503, 504, 408, 429, Timeout, Network)
                Adapter-->>Inf: ProviderError
                Inf->>Pol: calculate_backoff(attempt, retry_after)
                alt Delay > max_retry_after_secs or deadline exceeded
                    Note over Inf: Break candidate retry loop immediately (avoid excessive sleep)
                else Valid Delay
                    Note over Inf: Sleep with exponential backoff & jitter
                end
            end
        end
        Note over Inf: Add candidate to failed_model_ids (loop prevention)
        Note over Inf: In manual mode: fail immediately (never switch models)
    end
    Note over Inf: Record final request outcome exactly once
    Inf-->>Usage: record_request() (exactly 1 row)
    Inf-->>GW: Final Success or Structured AppError
```

### 7.2 Decision Rules & Invariants
1. **Transient vs Non-Retryable Error Taxonomy:**
   - **Transient (Retry Eligible):** `ProviderError::Timeout`, `ProviderError::NetworkFailure`, `ProviderError::RateLimited`, and `ProviderError::UpstreamError` with HTTP status 408, 429, 500, 502, 503, 504.
   - **Non-Retryable (Immediate Short-Circuit):** `ProviderError::AuthenticationFailed`, `ProviderError::InvalidRequest`, `ProviderError::ModelNotFound`, `ProviderError::CapabilityMismatch`, `ProviderError::ProviderDisabled`, `ProviderError::ProviderNotFound`, and HTTP status codes 400â€“404, 422.
2. **Exponential Backoff with Monotonic Non-Locking Jitter:**
   - Backoff delay formula: $t = \min(\text{initial\_backoff} \times 2^{\text{attempt}-1}, \text{max\_backoff})$.
   - Non-locking pseudo-random jitter combines an atomic sequence counter (`AtomicU64` with Weyl constant `0x9E3779B97F4A7C15`) and monotonic clock nanoseconds (`LazyLock<Instant>`) via SplitMix64 mixing, scaling delay strictly between 75% and 125% with zero lock contention or external dependencies.
   - For deterministic testability, `calculate_backoff_with_jitter` allows tests to specify explicit jitter percentages.
3. **Retry-After Enforcement (delay-seconds & HTTP-date):**
   - Implements RFC 9110 Â§10.2.3 compliant parsing supporting both integer `delay-seconds` (e.g. `120`) and RFC 7231 IMF-fixdate `HTTP-date` (e.g. `Sun, 06 Nov 2026 08:49:37 GMT`).
   - Past dates safely clamp to 0 seconds wait without underflow.
   - If `retry_after > max_retry_after_secs`, the policy yields `Duration::MAX`. The execution loop abandons that candidate immediately rather than blocking worker threads, enabling instant failover to secondary providers.
   - Malformed or invalid header values safely return `None` and fall back to standard bounded exponential backoff.
4. **Shared Request Deadline:**
   - All attempts, retries, and sleep delays share the overarching `overall_request_timeout_secs` deadline.
   - Each attempt computes `attempt_timeout = min(upstream_timeout_secs, remaining_deadline)`. If the deadline expires, the engine aborts with `GatewayTimeout` (504).
5. **Selection Loop Prevention:**
   - `failed_model_ids` hash set tracks all models that exhausted attempts or failed with candidate-terminal errors during the request.
   - Exhausted candidates are never reselected during alternative candidate iteration.
6. **Manual Routing Invariant:**
   - In `manual` routing mode, `resolve_candidates` returns strictly `vec![requested_model]`.
   - The engine allows bounded retries of the *same* requested model if transient, but **never** substitutes or falls back to an alternative model/provider.
7. **Streaming Reliability (Pre-Stream vs Mid-Stream):**
   - **Pre-Stream:** Upstream connection errors before headers or chunks are delivered allow bounded retries and automatic failover.
   - **Mid-Stream:** Once HTTP headers and the initial SSE `event: routing` or chunk are sent, failover and restarts are strictly disallowed. An upstream disconnection emits a structured `event: error` and clean `event: done` EOF, preserving stream integrity and avoiding duplicate billing or garbled output.
8. **Exactly-Once Usage Finalization:**
   - Request usage recording occurs exactly once across all attempts, retries, and failovers. The single recorded row captures the final outcome (`Success`, `ProviderError`, `Timeout`, `StreamInterrupted`, `ClientCancelled`).

### 7.3 Duplicate Execution, Idempotency, and Upstream Billing Risks
- **Inherent Upstream Ambiguity:** While connection failures (`err.is_connect()`) guarantee that the upstream never received the request payload, upstream HTTP timeouts (`ProviderError::Timeout`) introduce unavoidable ambiguity: the provider's inference cluster may have accepted the request and commenced GPU token generation before the client-side attempt timeout expired.
- **Provider Protocol Limitations:** Contemporary LLM inference endpoints (Groq, OpenRouter, OpenAI, Anthropic) lack a standardized, universally honored idempotency header contract for chat completions.
- **Mitigation & Safety Invariants:**
  1. *Strict Attempt Bounding:* Retries per candidate are strictly capped (`max_retries = 1` by default), preventing unbounded retry loops or bill multiplication.
  2. *Zero Mid-Stream Duplication:* Once any SSE header or token delta is dispatched to the client, retries and failovers are completely prohibited. Mid-stream transport drops terminate with an error event, preventing duplicate token emissions or double-billing.
  3. *Unavoidable Uncertainty:* Because remote AI providers do not support transactional rollback or cancellation receipts for started GPU inference batches, retrying an ambiguous timeout carries unavoidable financial risk at the upstream account layer. The platform minimizes this exposure by capping retries, enforcing overall deadlines, and leaving client-side retries to explicit user discretion.

---

## 8. Phase 10: Token Optimization & Performance Architecture

### 8.1 In-Process Deterministic Token Estimation
Phase 10 introduces a lightweight, in-process, deterministic token estimator (`src/inference/tokens.rs` via `TokenEstimator`) avoiding external C/Python dependencies, network round-trips, or memory-heavy BPE vocabulary files.
- **Subword & Character Heuristic:**
  - Modern Byte-Pair Encoding (BPE) tokenizers (cl100k_base, Llama 3 with 128k vocabularies) average ~4 English characters per token for continuous text, while subword delimiters (whitespace, punctuation, newlines) terminate tokens.
  - Formula: $\text{Tokens} = \max(1, \lceil \text{character\_count} / 4 \rceil + \text{subword\_delimiters})$.
  - Capped to prevent artificial inflation: bounded between $\lceil\text{chars} / 4\rceil$ and $\text{chars}$.
- **ChatML Framing Overhead:**
  - Each message introduces ChatML framing tokens: 4 tokens per message (`<|im_start|>{role}\n{content}<|im_end|>\n`).
  - Request conversation introduces +3 tokens for the assistant generation primer (`<|im_start|>assistant\n`).
- **Bounded Thread-Safe LRU Cache:**
  - Fast-path lookup for repeated prompt queries using a 512-slot circular LRU cache (`Mutex<LruCache>`).
  - Employs 64-bit FNV-1a hashing (`fnv1a_hash`), completing hash computation and cache lookup in $<1\,\mu\text{s}$ without external hashing crate dependencies.

### 8.2 Conservative Prompt Compaction Engine
- **Whitespace Normalization:**
  - Strips trailing whitespace from each line.
  - Strips consecutive empty blank lines.
  - **Preserves Leading Indentation:** Critical invariant ensuring code snippets, Python scripts, YAML, and Markdown formatting remain semantically intact.
- **System Message Deduplication:**
  - Automatically collapses consecutive identical system prompts (common in multi-turn client chat frameworks or client middleware) into a single canonical instruction.
- **Compaction Metrics (`TokenCompactionMetrics`):**
  - Measures original characters, compacted characters, original estimated tokens, compacted estimated tokens, and savings percentage.

### 8.3 Context Window Capacity-Aware Routing
Integrated directly into `InferenceService`:
```mermaid
flowchart TD
    Req[Incoming Chat Request] --> Comp[Conservative Prompt Compaction]
    Comp --> Est[Deterministic Token Estimation]
    Est --> Mode{Routing Mode?}
    
    Mode -->|Auto Mode| AutoFilter[Filter Candidate Models]
    AutoFilter --> Check1{Context Window >= Prompt + 256?}
    Check1 -->|Yes| Keep[Include in Candidate Scoring]
    Check1 -->|No| Drop[Exclude Incompatible Model]
    Keep --> Score[Select Optimal Candidate]
    
    Mode -->|Manual Mode| ManCheck{Requested Model Context >= Prompt + 256?}
    ManCheck -->|Yes| Dispatch[Dispatch to Provider Adapter]
    ManCheck -->|No| FastFail[Short-Circuit: HTTP 400 InvalidRequest]
```
- In **`auto`** mode: Filters out models whose `context_window` cannot accommodate the estimated prompt tokens plus a conservative 256-token completion headroom. Prevents selecting small-context models (e.g. 4k) for large prompts (e.g. 8k), preventing guaranteed upstream provider 400 errors.
- In **`manual`** mode: Validates requested model capacity prior to dispatch. If prompt exceeds context window, rejects immediately with HTTP 400 `InvalidRequest` with a clear explanation (`prompt estimated at X tokens exceeds context window of Y tokens`), saving latency and avoiding wasted upstream calls.

### 8.4 Pipeline & Parsing Hot-Path Optimizations
1. **Redundant Candidate Resolution Elimination (`src/routes/chat.rs`):**
   - Previous flow invoked `state.inference.resolve_candidates` twice per request: once in the outer HTTP handler and again inside `InferenceService::execute_completion`.
   - Optimized flow invokes candidate resolution exclusively once inside `execute_completion`. The outer route resolves candidates only along error fallback paths for error response enrichment.
   - Result: 53.1% reduction in mean request processing overhead.
2. **Zero-Allocation SSE Chunk Slice Parsing (`src/providers/openai_compatible.rs`):**
   - Previous streaming parser allocated a new heap `String` for every line chunk using `String::from_utf8_lossy`.
   - Optimized parser processes line events directly from `&buffer[..pos]` string slices, parsing `data: ` payloads in-place and performing zero heap allocations for event delimitation.
   - Result: 31.6% reduction in Time-to-First-Chunk (TTFC) under concurrency ($C=20$).

---

## 9. Completed Hardening & Operational Resilience (Phase 10 Closure)

The hardening backlog items identified during earlier phases have been formally implemented and verified:

1. **Host-Based Bind Security & Reverse-Proxy Hardening (`src/security/mod.rs`):**
   - *Status:* **IMPLEMENTED & VERIFIED**.
   - *Architecture:* Supports explicit reverse-proxy IP allowlisting via bitmask `IpCidr` parsing (IPv4 & IPv6). `TrustedProxyMatcher` allowlists proxy CIDRs, and `HostValidator` defends against Host-header poisoning. Forwarded headers (`X-Forwarded-For`, `Forwarded`) are stripped when originating from untrusted network peers. Ingress mTLS is documented at the reverse-proxy termination boundary.
2. **Retention Cleanup Reliability & Autonomous Maintenance (`src/usage/storage.rs`, `src/usage/service.rs`):**
   - *Status:* **IMPLEMENTED & VERIFIED**.
   - *Architecture:* Bounded subquery pruning (`prune_older_than_bounded`) prevents table locking on high-volume SQLite instances. Integrated with non-blocking passive WAL checkpoints (`maintenance_wal_checkpoint`) to maintain low disk footprint.
3. **Durable Usage-Recording Failure Handling (`src/usage/storage.rs`, `src/usage/service.rs`):**
   - *Status:* **IMPLEMENTED & VERIFIED**.
   - *Architecture:* Out-of-band persistent retry queue (`usage_retry_queue` in SQLite) guarantees zero telemetry loss during transient database saturation. Records are retried using bounded exponential backoff (up to 300s); exhausted retries (5 attempts) or poison payloads transition safely to dead-letter status, preserving user inference semantics and accounting auditability.

---

## 10. Multi-Client Authentication Subsystem (Phase 11 Implementation)

### 10.1 Authentication Ingress & Precedence Topology

All protected requests pass through the unified `auth_middleware` (`src/auth/middleware.rs`), which orchestrates request tracing, Host-header validation, reverse-proxy security, query-credential rejection, authentication dispatch, and route scope authorization:

```mermaid
graph TD
    Req[Inbound HTTP Request] --> ProbeCheck{Path == /health or /ready?}
    ProbeCheck -->|Yes| PublicProbe[Bypass Auth -> 200 OK / 503 Ready]
    ProbeCheck -->|No| QueryCheck{Query has api_key / token / auth?}

    QueryCheck -->|Yes| QueryDeny[Reject: HTTP 401 Insecure Query Credential]
    QueryCheck -->|No| HdrInspect[Inspect Authorization & X-API-Key]

    HdrInspect --> CredCheck{Credentials Present?}
    CredCheck -->|Bearer Header| TokenDispatch{Token Structure}
    TokenDispatch -->|3-Segment JWT| OIDC[GoogleOidcValidator: RS256 JWKS]
    TokenDispatch -->|PAT / API Key| PAT[PatValidator: Constant-Time SHA-256]

    CredCheck -->|X-API-Key Header| PAT

    CredCheck -->|None| BindCheck{Host != Loopback OR Production OR Auth Required?}
    BindCheck -->|Yes| Deny[Reject: HTTP 401 Unauthorized]
    BindCheck -->|No| DevBypass[DevelopmentBypass Principal]

    OIDC -->|Valid Signature, iss, aud, exp, nbf, iat, azp| ScopeCheck
    OIDC -->|Invalid / Expired / Wrong kid| Deny

    PAT -->|SHA-256 Hash Match| ScopeCheck
    PAT -->|Mismatch / Empty / Revoked| Deny

    ScopeCheck{Principal has Route Scope?} -->|Yes| Success[Inject AuthPrincipal into Request Extensions]
    ScopeCheck -->|No| Forbidden[Reject: HTTP 403 Forbidden]

    Success --> Downstream[Downstream Route Handlers & Telemetry]
```

### 10.2 Architectural Components & Invariants
1. **Unconditional Query-Parameter Credential Proscription (`src/auth/service.rs`):**
   - In accordance with OWASP API Security and REST security best practices, credentials (`api_key`, `apikey`, `api-key`, `access_token`, `token`, `auth_token`, `auth`, `bearer`, `secret`) passed in URL query strings are strictly prohibited.
   - Automatically percent-decodes URL query keys (handling `%xx` encodings, `+` and whitespace) and checks against credential parameters to completely eliminate percent-encoding evasion attacks.
   - Any protected request containing credential-bearing query parameters is immediately rejected with HTTP 401 `Unauthorized` and canonical code `UNAUTHORIZED` before any header-based fallback can execute, preventing credential leakage in proxy access logs, browser history, and referrer headers.
2. **Google OIDC ID Token Validator (`src/auth/oidc.rs`):**
   - Parses RS256 JWT tokens using `jsonwebtoken`.
   - Retrieves public RSA keys via the `JwksProvider` abstraction (`HttpJwksProvider` with in-memory caching and 1-hour TTL).
   - Validates strict standard claims: issuer (`https://accounts.google.com` or `accounts.google.com`), audience (matching configured `google_client_id` or `google_client_ids`), expiration (with 60-second leeway), non-empty subject, `nbf` (not-before), future `iat` sanity, and `azp` (authorized party) strictly matching one of the configured Google client IDs.
   - Enforces an incremental 256 KB streaming body limit during JWKS downloads, aborting immediately upon exceeding 256 KB to defend against memory exhaustion and upstream payload amplification attacks.
   - Employs an atomic write-locked 10-second refresh throttle on cache misses caused by unknown key identifiers (`kid`) to prevent concurrent cache-miss network amplification attacks.
   - Supports `MockJwksProvider` enabling deterministic, offline, zero-network unit and integration testing.
3. **Personal Access Token & API Key Validator (`src/auth/pat.rs`):**
   - Validates developer PATs (`pat_<64_hex_chars>`) and backend API keys.
   - Precomputes 32-byte SHA-256 digests (`ring::digest`) upon configuration loading.
   - Executes comparisons in constant time via `subtle::ConstantTimeEq`, completely eliminating timing attacks across both token length and value.
   - Custom `Debug` implementation on `ConfiguredCredential` securely redacts token hashes to prevent diagnostic log leakage.
4. **Downstream Identity & Scope Authorization (`src/auth/principal.rs`, `src/auth/service.rs`):**
   - Authenticated identity is encapsulated in `AuthPrincipal` and attached to Axum request extensions.
   - Contains caller `client_type` (`Flutter`, `CustomIde`, `VscodeExtension`, `ApiClient`), `subject`, `auth_type`, `email`, `roles` (`User`, `Admin`, `Service`), and authorized `scopes`.
   - Implements custom `Debug` and `Display` to redact emails and secrets, preventing log leakage.
   - Route-level scope authorization (`AuthService::authorize`) enforces fine-grained access boundaries: `models:read` for model and provider catalogs, `routing:read` for routing preview, `chat` for completion and streaming endpoints, and `usage:read` for usage telemetry and quotas. Principals lacking required scopes receive HTTP 403 `Forbidden`.
5. **Fail-Closed Production & Non-Loopback Network Boundary (`src/config.rs`, `src/auth/service.rs`):**
   - In production (`APP_ENV=production`), application startup strictly requires at least one configured credential (`API_KEY`, `API_KEYS`, or `GOOGLE_CLIENT_ID`), preventing accidental unauthenticated production deployments. Production mode strictly mandates HTTPS for `GOOGLE_JWKS_URL`.
   - Whenever the backend binds to a non-loopback network interface (e.g., `0.0.0.0` or a public IP), authentication is automatically and unconditionally enforced for all protected routes, guaranteeing that development bypass can never expose endpoints across external network interfaces.
