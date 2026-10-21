# CHANGELOG.md — Project Activity & Version History

All notable project changes, architectural decisions, and document synchronizations are recorded chronologically in this file.

## 2026-10-21 — Phase 11: Production-Grade Security Audit, Remediation, Header Hardening, and Multi-Audience azp Enforcement

### Audited, Corrected & Verified
- **OpenID Connect Core 1.0 Multi-Audience `azp` Enforcement (`src/auth/oidc.rs`):**
  - Enforced OIDC Core 1.0 §3.1.3.7 rule 3: tokens containing multiple audiences (`claims.aud` as an array with > 1 elements) MUST contain an authorized party (`azp`) claim.
  - Tokens missing `azp` under multi-audience configurations are immediately rejected with `OidcError::InvalidAudience`.
- **JWKS Concurrency Cache Re-check on Throttle (`src/auth/oidc.rs`):**
  - In `HttpJwksProvider::get_decoding_key`, re-checks `self.cache.read().await` when the 10-second unknown-`kid` throttle is active before returning `Err(OidcError::UnknownKeyId)`.
  - Safely handles concurrent in-flight requests during legitimate Google key rotations without false-negative rejections.
- **Header Parsing Security & Conflicting Scheme Defense (`src/auth/service.rs`):**
  - Prohibited duplicate `Authorization` headers (`headers.get_all("authorization").iter().count() > 1`) and duplicate `X-API-Key` headers (`headers.get_all("x-api-key").iter().count() > 1`), returning HTTP 401 `Unauthorized`.
  - Prohibited simultaneous conflicting credential headers (both `Authorization` and `X-API-Key` present), returning HTTP 401 `Unauthorized` to prevent ambiguous or contradictory identity resolution.
  - Hardened header value decoding to validate UTF-8 encoding, failing closed with HTTP 401 on non-UTF-8 bytes rather than silently losing the header.
- **Credential Length Bounding & Resource Exhaustion Defense (`src/auth/service.rs`):**
  - Established `AuthService::MAX_CREDENTIAL_LEN = 8192` (8 KiB) ceiling on Bearer tokens and API keys.
  - Protects token parsing, hashing, and string allocation against memory exhaustion and regex/parsing Denial of Service attacks.
- **Comprehensive Verification & Test Suite Expansion (`tests/authentication_test.rs`, `src/auth/oidc.rs`, `src/auth/service.rs`):**
  - Added Test 25: Multi-audience OIDC token requiring `azp`.
  - Added Test 26: Duplicate and conflicting header rejection.
  - Added Test 27: Oversized credential rejection (>8192 bytes for both Bearer and API keys).
  - Added Test 28: Non-UTF-8 header rejection.
  - Added Test 29: Concurrent OIDC, unknown-kid, and API-key request safety under concurrent task execution (`tokio::task::JoinSet`).
  - Added 8 unit tests in `src/auth/oidc.rs` and `src/auth/service.rs` validating audience matching, JWKS provider constants, unconfigured client IDs, route scope resolution, and query credential detection.
  - Total automated tests increased to **325 passing tests** across 17 test suites (89 unit + 236 integration across 16 integration binaries, 0 failures, 1 ignored load benchmark).
  - Full quality gates: `cargo fmt --check` clean, `cargo check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo audit -n` clean (0 advisories across 183 crates), `gitleaks detect -v` clean (0 leaks), `cargo build --release` passing.


## 2026-10-21 — Forensic Security Remediation: Query Percent-Decoding, OIDC azp Validation, Streaming JWKS Limits, and Configuration Precedence

### Audited, Corrected & Verified
- **Percent-Encoded Query Parameter Credential Defense (`src/auth/service.rs`):**
  - Implemented `percent_decode` helper to safely decode URL hex escape sequences (`%xx`) and `+`/whitespace before credential detection.
  - Hardened `AuthService::contains_credential_query` to normalize query parameters, split on both `&` and `;`, and inspect canonicalized keys against an expanded credential keyword list (`api_key`, `apikey`, `api-key`, `access_token`, `accesstoken`, `access-token`, `token`, `auth_token`, `authtoken`, `auth-token`, `auth`, `bearer`, `secret`, `client_secret`, `client-secret`).
  - Guarantees that evasion techniques utilizing percent-encoding (e.g., `?api%5fkey=...`) cannot bypass query-credential rejection or leak secrets into server logs.
- **Google OIDC `azp` (Authorized Party) Claim Enforcement (`src/auth/oidc.rs`):**
  - Resolved logic flaw in `GoogleOidcValidator::verify` where `azp` validation was bypassed when `claims.matches_audience(...)` was true.
  - Enforced that if an `azp` claim is present in a Google ID token, it MUST strictly match configured Google Client IDs, preventing token re-presentation attacks by unauthorized cross-client applications.
- **Streaming Chunk-by-Chunk JWKS Download Ceiling (`src/auth/oidc.rs`):**
  - Replaced unconstrained `response.bytes().await` with incremental streaming chunk inspection (`response.chunk().await`).
  - Bounded downloads strictly at 256 KB (`MAX_JWKS_RESPONSE_BYTES = 262_144`); immediately aborts the connection if an upstream server transmits exceeding bytes, neutralizing memory exhaustion and payload amplification from unbounded chunked transfers.
- **Atomic Write-Lock JWKS Unknown-`kid` Throttling (`src/auth/oidc.rs`):**
  - Updated `HttpJwksProvider::get_decoding_key` to check and record `last_fetch_attempt` under an atomic write lock *before* dispatching outbound network requests.
  - Eliminates thundering-herd race conditions and prevents outbound network amplification under concurrent requests with invalid key identifiers.
- **Configuration Precedence & Production Validation (`src/config.rs`):**
  - Fixed `Config::is_auth_required` precedence: server binding to non-loopback addresses (`!is_loopback_host()`) or running in production (`APP_ENV=production`) unconditionally requires authentication, ignoring `auth_required = Some(false)`.
  - Added strict validation in `Config::validate()` to reject `auth_required = Some(false)` when `APP_ENV=production` with `ConfigError::InvalidValue`.
  - Enhanced loopback host normalization to support IPv6 bracket notation (e.g., `[::1]`) and case-insensitive `"localhost"`.
- **Integration Test Expansion (`tests/authentication_test.rs`):**
  - Added Test 20: Percent-encoded query parameter credential rejection (`?api%5fkey=secret` returns HTTP 401 `Unauthorized`).
  - Added Test 21: Google OIDC `azp` claim verification rejecting tokens with unauthorized presenters even when audience matches.
  - Added Test 22: Production configuration startup rejection of `AUTH_REQUIRED=false`.
  - Added Test 23: Loopback host normalization for `[::1]`, `::1`, and case-insensitive `LOCALHOST`.
  - Added Test 24: `Config::is_auth_required` precedence in production and non-loopback configurations.
  - Suite expanded from 19 to 24 tests; full backend test suite increased to **312 passing tests** (81 unit + 231 integration across 16 integration binaries, 0 failures, 1 ignored load benchmark).

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **312 automated tests passing** across 17 test suites (81 unit + 231 integration) with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 183 dependencies scanned).
- `cargo build --release`: Passed with exit code 0.
- `gitleaks detect -v`: Passed with exit code 0 (0 leaks detected).
- Phase 12 (Authorization & Secret Management) remains strictly on hold.

---

## 2026-10-21 — Phase 11: Security Audit Remediation, Query Credential Rejection, Scope Authorization, and Non-Loopback Bind Protection

### Audited, Corrected & Verified
- **Unconditional Query-Parameter Credential Proscription (`src/auth/service.rs`, `src/auth/middleware.rs`):**
  - Addressed confirmed security gap: prevented sensitive credentials from appearing in URL query strings (`api_key`, `apikey`, `access_token`, `token`, `auth_token`, `auth`, `bearer`).
  - Implemented `AuthService::contains_credential_query`: protected endpoints immediately reject query-parameter credentials with HTTP 401 `Unauthorized` and canonical code `UNAUTHORIZED` prior to checking headers.
  - Mitigates credential leakage into web server logs, proxy access logs, browser history, and HTTP `Referer` headers.
- **Strict Non-Loopback Network Bind Protection (`src/config.rs`, `src/auth/service.rs`):**
  - Resolved external interface exposure risk: updated `Config::is_auth_required` and `AuthService::authenticate` to unconditionally enforce authentication whenever the server binds to a non-loopback host interface (`!is_loopback_host()`, such as `0.0.0.0` or public IPs).
  - Guarantees that unauthenticated development bypass can never expose endpoints across external network interfaces, even if `AUTH_REQUIRED=false` or `APP_ENV=development`.
- **Route-Level Scope Authorization Enforcement (`src/auth/service.rs`, `src/auth/middleware.rs`):**
  - Implemented `AuthService::required_scope_for_route` and `AuthService::authorize(&principal, path)`.
  - Mapped route paths to mandatory scopes:
    - `/api/v1/models` and `/api/v1/providers` require `models:read`
    - `/api/v1/routing/preview` requires `routing:read`
    - `/api/v1/chat/*` requires `chat`
    - `/api/v1/usage/*` and `/api/v1/quotas` require `usage:read`
  - Rejects under-scoped callers with HTTP 403 `Forbidden` and canonical error code `FORBIDDEN`.
- **Google OIDC ID Token & JWKS Hardening (`src/auth/oidc.rs`, `src/config.rs`):**
  - Added bounded 256 KB response body limit (`MAX_JWKS_RESPONSE_BYTES = 262_144`) on JWKS downloads to defend against upstream payload amplification and memory exhaustion.
  - Implemented a 10-second refresh throttle on unknown key identifiers (`kid`) to prevent cache-miss network request amplification attacks.
  - Enforced HTTPS for `GOOGLE_JWKS_URL` in production environments (`APP_ENV=production`).
  - Added claim validation for `nbf` (not-before), future `iat` sanity check, and `azp` (authorized party) verification against configured Google Client IDs.
- **Diagnostic Secret Redaction (`src/auth/pat.rs`):**
  - Implemented custom `std::fmt::Debug` on `ConfiguredCredential` to securely redact precomputed token hashes (`"[REDACTED_HASH]"`).
- **Comprehensive Integration Test Expansion (`tests/authentication_test.rs`):**
  - Added Test 13: Query-parameter credential rejection (`api_key`, `access_token`, `token` in URL query string rejected with 401).
  - Added Test 14: Route-level scope authorization (insufficient scope returns HTTP 403 Forbidden).
  - Added Test 15: External bind protection (`0.0.0.0` rejects unauthenticated requests even in development mode).
  - Added Test 16: Client-type header isolation (spoofed `X-Client-Type` cannot grant admin privileges).
  - Added Test 17: Production HTTPS JWKS URL validation (insecure HTTP URL rejected in production).
  - Added Test 18: Non-loopback host `is_auth_required` configuration check.
  - Added Test 19: Trailing slash public probe path normalization (`/health/`, `/ready/`).
  - Integration suite expanded to 19 comprehensive tests; full backend test suite increased to **306 passing tests** (81 unit + 225 integration across 17 test suites, 0 failures, 1 ignored load benchmark).

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **306 automated tests passing** across 17 test suites with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 183 dependencies scanned).
- `cargo build --release`: Passed with exit code 0.
- Phase 12 (Authorization & Secret Management) remains strictly on hold.

---

## 2026-10-21 — Phase 11: Authentication & Authorization Complete & Verified

### Implemented & Verified
- **Multi-Client Authentication Subsystem (`src/auth/`):**
  - Implemented decoupled, client-independent authentication service supporting Flutter mobile applications, Custom AI IDEs, VS Code editor extensions, and automated API consumers.
  - **Google Sign-In OIDC ID Token Verification (`src/auth/oidc.rs`):**
    - Cryptographic RS256 JWT signature verification against Google's public JSON Web Key Sets (JWKS) via `jsonwebtoken`.
    - Strict validation of standard claims: `iss` (`https://accounts.google.com` or `accounts.google.com`), `aud` against configured Google Client IDs, expiration timestamp (`exp` with 60-second RFC 7519 leeway), and non-empty subject (`sub`).
    - In-memory JWKS key caching with 1-hour TTL and bounded 5-second fetch timeout (`HttpJwksProvider`).
    - Deterministic offline testability via `MockJwksProvider` supporting mock RSA keypairs with zero live external network dependencies.
  - **Constant-Time Personal Access Token & API Key Verification (`src/auth/pat.rs`):**
    - Cryptographically secure token verification using 32-byte SHA-256 digests (`ring::digest`) and constant-time equality checks (`subtle::ConstantTimeEq`).
    - Eliminates character- and length-dependent timing side channels across candidate tokens.
    - Secure PAT generation helper generating `pat_<64_hex_chars>` using OS secure random entropy (`ring::rand::SystemRandom`).
  - **Request Identity & Authorization Propagation (`src/auth/principal.rs`):**
    - Strongly-typed `AuthPrincipal` inserted into Axum request extensions, carrying `subject`, `auth_type` (`GoogleOidc`, `PersonalAccessToken`, `ApiKey`, `DevelopmentBypass`), `client_type` (`Flutter`, `CustomIde`, `VscodeExtension`, `ApiClient`), `email`, `roles` (`User`, `Admin`, `Service`), and authorized `scopes`.
    - Zero-leak `Debug` and `Display` implementations that redact email addresses and sensitive secrets.
  - **Strict Precedence & Zero-Fallback Rule (`src/auth/service.rs`):**
    - 3-segment JWT tokens route strictly to the Google OIDC validator.
    - Non-JWT tokens and `X-API-Key` headers route strictly to PAT/API key validators.
    - Invalid or expired credentials immediately return HTTP 401 Unauthorized and NEVER fall back to weaker authentication or developer bypass.
  - **Unified Authentication Middleware (`src/auth/middleware.rs`):**
    - Single consolidated Axum middleware enforcing request ID correlation (`X-Request-ID`), Host header validation (`ALLOWED_HOSTS`), reverse-proxy spoofing defenses (`TRUSTED_PROXIES`), and identity injection.
  - **Fail-Closed Production Safety (`src/config.rs`):**
    - In production mode (`APP_ENV=production`), application startup fails closed with `ConfigError::InvalidValue` if no valid `API_KEY`, `API_KEYS`, or `GOOGLE_CLIENT_ID` is present.
    - Enforced minimum 8-character length on all configured API keys.
  - **External Bind Protection for Operational Telemetry:**
    - Usage and quota endpoints (`/api/v1/usage/*`, `/api/v1/quotas`) unconditionally require authentication whenever the server binds to a non-loopback host interface.

### Verification Results
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **299 automated tests passing** across 17 test suites with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 183 dependencies scanned).
- `cargo build --release`: Passed with exit code 0.
- Phase 12 (Authorization & Secret Management) remains strictly on hold.

---

## 2026-10-21 — Comprehensive Token Estimation Audit, Tokenizer Correction & Production Readiness

### Audited, Corrected & Verified
- **Centralized Model-to-Tokenizer Mapping (`src/inference/tokens.rs`):**
  - Eliminated dangerous broad substring matching (`.contains("gpt")`, `.contains("o1")`, etc.) which falsely mapped unrelated models (`auto1`, `echo1`, `audio3`, `my-custom-gpt`) and misclassified newer OpenAI models (`gpt-4.1`, `gpt-4.5`, `gpt-5` as `cl100k` and `o4-mini` as generic).
  - Implemented authoritative mapping engine `resolve_model_tokenizer(model_id, registry)` incorporating verified evidence from OpenAI's `tiktoken/model.py`:
    - `o200k_base` verified: `o1`, `o3`, `o4-mini`, `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.5`, `gpt-5`, `chatgpt-4o-latest`, and prefixes `o1-`, `o3-`, `o4-mini-`, `gpt-4.1-`, `gpt-4.5-`, `gpt-5-`, `ft:gpt-4o`.
    - `cl100k_base` verified: `gpt-4`, `gpt-4-turbo`, `gpt-3.5-turbo`, and prefixes `gpt-4-`, `gpt-3.5-turbo-`, `ft:gpt-4`, `ft:gpt-3.5-turbo`.
    - Calibrated non-BPE prefixes: `llama-`, `mistral-`, `gemma-`, `qwen-`, `deepseek-`.
    - Explicit reference aliases: `fast-chat-v1` -> `Llama`, `deep-reasoner-v1` -> `DeepSeek`, `balanced-assistant-v1` -> `Mistral`.
    - Namespace stripping: safely processes `openai/*`, `groq/*`, `openrouter/*`.
    - Safe fallback: unknown models and negative substring lookalikes strictly resolve to `MappingKind::Unknown` and `TokenizerFamily::Generic`.
- **Truthful Exactness Semantics (`src/inference/tokens.rs`):**
  - Strictly separated exact raw-text BPE tokenization from multi-message chat estimation.
  - Raw-text BPE tokenization (`estimate_text_detailed`) sets `is_exact = true`, `method = ExactTokenizer`, and `raw_text_exact = Some(true)`.
  - Multi-message chat requests (`estimate_chat_detailed`, `estimate_request_context_safe`) set `is_exact = false`, `method = EstimatedChatFraming`, and `raw_text_exact = Some(true)` because message delimiters (+3 per message, +3 assistant primer, +12 schema overhead) are heuristic framing conventions, not exact wire tokenization.
- **Effective-Message Consistency & Output Token Validation (`src/inference/mod.rs`):**
  - Reconciled prompt compaction with candidate admission: `resolve_candidates` now evaluates effective compacted messages (`effective_messages`) rather than raw messages, ensuring that admission checks and dispatched payloads are identical.
  - Differentiated output-token limit violation from context-window exhaustion in auto-routing: if requested `max_tokens` exceeds all available models' `max_output_tokens`, it returns a specific error (`Requested max_tokens ({requested}) exceeds maximum output token limit of all available operational models ({max})`) rather than incorrectly claiming context window exhaustion.
- **Comprehensive Reference & Regression Test Matrix (`tests/model_aware_tokenization_and_admission_test.rs`):**
  - Added Category 16: Table-driven model-mapping matrix covering all catalog models, investigated next-gen OpenAI models, and negative lookalikes.
  - Added Category 17: Reference tokenizer test suite verifying exact `cl100k_base` and `o200k_base` counts against official `tiktoken` provenance across English, Rust code, Python code, JSON, long identifiers, Urdu script (demonstrating 52 -> 22 token reduction in o200k), emojis, and punctuation.
  - Added Category 18: Auto-routing candidate admission and output limit discrimination tests.
  - Added Category 19: Message optimization consistency in candidate admission and routing.
  - Test suite count increased to **279 passing tests** (16 test suites, 0 failures, 1 ignored load benchmark).

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **279 automated tests passing** across 16 test suites with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 170 dependencies scanned).
- `gitleaks detect -v`: Passed with exit code 0 (12 commits scanned, 0 leaks found).

---

### Added & Hardened
- **Native Offline BPE Tokenizer Integration (`tiktoken-rs`):**
  - Integrated `tiktoken-rs = { version = "0.12", default-features = false }` providing pure Rust, zero-network, offline Byte Pair Encoding for OpenAI models.
  - Implemented thread-safe, lazily initialized static singletons (`CL100K_BPE` and `O200K_BPE`) via `std::sync::LazyLock<Option<CoreBPE>>`. Tokenizer tables are compiled and loaded exactly once on first access, eliminating per-request loading overhead.
  - Exact tokenization for GPT-4 (`cl100k_base`) and GPT-4o / o1 / o3 (`o200k_base`), matching official OpenAI reference tokenizers with verified counts (e.g. `fn compute(x: i32) -> i32 { x * 2 + 1 }` = exactly 19 tokens).
- **Explicit Model Capability Hierarchy (`src/inference/tokens.rs`):**
  - Added `TokenizerCapability` enum with `Exact`, `Approximate`, and `ProviderReported` tiers.
  - Model family mapping: `TokenizerFamily::Gpt` operates with `TokenizerCapability::Exact`, reporting `is_exact: true`, `method: EstimationMethod::ExactTokenizer`, and `safety_margin_applied: 0`.
  - Non-GPT models (`Llama`, `Mistral`, `Gemma`, `Qwen`, `DeepSeek`, `Generic`) operate with `TokenizerCapability::Approximate`, reporting `is_exact: false`, `method: EstimationMethod::CalibratedFamilyHeuristic` or `ConservativeFallback`, and a conservative +15% safety headroom (minimum 8 tokens) in `estimate_request_context_safe`.
  - Collision-safe LRU caching (`CacheKey { text, family, bpe_variant }`) preventing cross-model or cross-vocabulary cache collisions.
- **Accurate ChatML Message Framing & Tool Overhead Calculation:**
  - Implemented exact per-message framing (3 tokens per message for `<|im_start|>{role}\n{content}<|im_end|>\n` + role tokens + content tokens) and 3 tokens assistant primer (`<|im_start|>assistant\n`).
  - Added `estimate_tools_tokens_for_model` correctly encoding tool JSON schemas with framing overhead.
- **Strict Context-Window Admission Safety (`src/inference/mod.rs`):**
  - Prohibited silent clamping of `req.max_tokens`: if requested `max_tokens > model.max_output_tokens`, the request is immediately rejected with HTTP 400 `InvalidRequest`.
  - Enforced overflow-safe checked arithmetic (`estimated_prompt_tokens.checked_add(completion_allowance)`) preventing integer overflow.
  - Rejection before provider dispatch: if `prompt + completion_allowance > model.context_window`, returns detailed rejection error.
  - Model-aware candidate filtering in automatic routing: evaluates candidate models against their specific output limit and model-aware context requirement, rather than using a single generic estimate.
  - Replaced un-modeled prompt optimization in `execute_completion` and `execute_stream` with `optimize_messages_for_model`.
- **Comprehensive Test Coverage (`tests/model_aware_tokenization_and_admission_test.rs` & `tests/multilingual_token_safety_test.rs`):**
  - Added 15 comprehensive automated test categories covering short English, long paragraphs, Urdu/multilingual scripts, mixed-language dialogues, code (Rust, JS, Python), nested JSON, long unbroken strings/hashes/UUIDs/Base64, emojis, ChatML multi-turn framing, tool schemas, empty/large inputs, boundary fits, 1-token-over-limit rejection, output token limit validation, unsupported model fallback, and overflow-safe arithmetic.
  - Test suite count increased from 242 to 258 passing tests with 0 failures, 0 warnings.

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **258 automated tests passing** across 16 test suites with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 170 dependencies scanned).
- `gitleaks detect`: Passed with exit code 0 (11 commits scanned, 0 leaks found).

---

## 2026-10-21 — Independent Audit, Dependency Remediation, and Durability Verification

### Remediated & Hardened
- **Dependency Vulnerability Remediation (RUSTSEC-2026-0382 & RUSTSEC-2026-0381):**
  - Upgraded `rustls-webpki` to `v0.103.16` resolving intermediate certificate verification vulnerability.
  - Upgraded `webpki-roots` to `v1.0.10` resolving distrusted emSign intermediate authority vulnerability.
  - Re-ran `cargo audit -n` across all 160 dependencies: 0 vulnerabilities found.
- **Repeatable Gitleaks Secret Scanning Integration (`.gitleaks.toml`):**
  - Installed and verified `gitleaks v8.24.0` across 8 Git commits and full repository history (~4.98 MB scanned).
  - Added targeted allowlist rules in `.gitleaks.toml` for synthetic error sanitization test fixtures.
  - Verified 0 secrets committed across working tree and historical commits.
- **Durable Disk Spool Hardware Synchronization & Idempotency (`src/usage/spool.rs`):**
  - Added `file.sync_data()` after buffer flushes in `append_record_sync` and temporary file rewriting, ensuring physical kernel-to-hardware disk synchronization (FlushFileBuffers / fsync).
  - Explicitly closed file reader handles before atomic temp-file replacement, preventing Windows file-lock conflicts.
  - Added dedicated test `test_spool_duplicate_replay_idempotency_prevents_double_counting` verifying `INSERT OR REPLACE` SQLite idempotency preventing duplicate usage records or double-counting.
- **Token Estimation Reference Model Mapping (`src/inference/tokens.rs`):**
  - Fixed tokenizer family resolution for registered reference models: mapped `fast-chat-v1` to `TokenizerFamily::Llama` and `balanced-assistant-v1` to `TokenizerFamily::Mistral`.
  - Added regression test `test_registered_reference_model_family_aliases`.
  - Added boundary tests for empty/whitespace prompts and mixed-script multi-language payloads (English + Urdu + Devanagari + Rust Code + Emojis).

### Verification
- `cargo fmt --check`: Passed with exit code 0.
- `cargo check`: Passed with exit code 0.
- `cargo clippy --all-targets --all-features -- -D warnings`: Passed with 0 warnings (exit code 0).
- `cargo test`: **243 automated tests passing** across 15 test suites with 0 failures, 0 errors, 1 ignored (controlled benchmark).
- `cargo build --release`: Passed with exit code 0 in 1m 22s.
- `cargo audit -n`: Passed with exit code 0 (1,348 advisories checked, 160 dependencies scanned).
- `gitleaks detect`: Passed with exit code 0 (8 commits scanned, 0 leaks found).

---

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
