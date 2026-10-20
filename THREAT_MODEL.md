# THREAT_MODEL.md — Comprehensive STRIDE Security Assessment

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Status:** Complete Threat Assessment & STRIDE Mitigation Matrix  
**Last Updated:** 2026-10-19

---

## 1. Security Context & Assets

### 1.1 Assets to Protect
1. **Upstream Provider API Keys & Credentials:** High-value target; leakage causes financial drain, service suspension, or unauthorized LLM consumption.
2. **User Identity & Authentication Tokens:** Google OIDC ID tokens, Personal Access Tokens (PATs), and API keys.
3. **Google Drive OAuth Tokens:** Refresh and access tokens possessing `drive.appdata` access.
4. **User Conversation Data & Uploads:** Sensitive prompts, intellectual property, proprietary code submitted by IDE/extension users, and uploaded documents.
5. **Backend Availability & Infrastructure Quotas:** CPU, memory, connection limits, and free-tier invocation budgets on Vercel and AI provider accounts.

### 1.2 Threat Actors
- **External Network Attackers:** Anonymous entities attempting unauthenticated access, DDoS, SSRF exploitation, or credential stuffing.
- **Malicious Authenticated Users:** Users attempting prompt injection, resource exhaustion, or cross-tenant data access.
- **Compromised Upstream AI Providers / Proxies:** Third-party APIs returning malicious payloads, prompt injections in tool responses, or unexpected status codes.
- **Malicious Third-Party GitHub Repositories:** Untrusted open-source AI wrappers containing backdoors or telemetry collectors.
- **Eavesdroppers on Client-to-Backend Transit:** Man-in-the-Middle (MitM) attacks intercepting unencrypted traffic.

### 1.3 Trust Boundaries
- **Boundary 1 (External Client to Backend Gateway):** Untrusted public internet to Axum HTTP gateway. Requires TLS 1.3, strict payload limits, and authentication.
- **Boundary 2 (Backend to Upstream AI Providers):** Outbound HTTPS requests to external provider APIs. Requires endpoint allowlisting and secret encapsulation.
- **Boundary 3 (Backend to Google Drive APIs):** Secure OAuth 2.0 communication over Google REST APIs using scoped tokens.
- **Boundary 4 (Process Memory Boundary):** Secret material in memory must be isolated from logs, error formatters, and telemetry spans.

---

## 2. STRIDE Threat Matrix & Verification Plans

### T-01: Upstream Provider API Key Leakage
- **STRIDE Category:** Information Disclosure
- **Attack Scenario:** Provider keys exposed via hardcoded code, Git commits, stack traces in error responses, or diagnostic logs.
- **Likelihood:** Medium | **Impact:** Critical | **Risk:** High
- **Mitigations:**
  1. Load keys exclusively from environment variables at startup.
  2. Encapsulate keys in custom `SecretString` types that override `Debug` and `Display` to print `[REDACTED]`.
  3. Pre-commit hooks and automated CI secret scanning.
  4. Response error formatters strictly forbid including raw upstream headers or request objects.
- **Residual Risk:** Low.
- **Verification Method:** Automated unit tests asserting that formatting `AppConfig` or `ProviderConfig` with `{:?}` never reveals raw keys; static secret scanning in CI.

### T-02: Server-Side Request Forgery (SSRF) via Dynamic Provider URLs
- **STRIDE Category:** Elevation of Privilege / Information Disclosure
- **Attack Scenario:** An attacker supplies a malicious provider URL pointing to cloud metadata services (`http://169.254.169.254`), loopback (`127.0.0.1`), or internal network IPs.
- **Likelihood:** Medium | **Impact:** Critical | **Risk:** High
- **Mitigations:**
  1. Provider URLs must come from trusted, server-side configuration, never from user request payloads.
  2. Implement an outbound address validator that resolves DNS and blocks:
     - IPv4 private ranges (RFC 1918: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
     - Loopback (`127.0.0.0/8`, `::1`)
     - Link-local and cloud metadata (`169.254.0.0/16`)
     - Non-HTTPS schemes (`file://`, `gopher://`, `http://`).
- **Residual Risk:** Very Low.
- **Verification Method:** Test suite submitting IP addresses in private/link-local ranges to the URL validator and asserting immediate rejection.

### T-03: Prompt Injection & Untrusted LLM Content Execution
- **STRIDE Category:** Tampering / Elevation of Privilege
- **Attack Scenario:** User content contains jailbreaks ("Ignore previous instructions...") or indirect prompt injection from fetched web pages / documents designed to hijack model behavior.
- **Likelihood:** High | **Impact:** Medium | **Risk:** Medium
- **Mitigations:**
  1. Maintain strict separation between system instructions and user-supplied prompt text in provider adapter payloads.
  2. Treat all LLM outputs as untrusted data: sanitize markdown/HTML before rendering in clients.
  3. Prevent models from emitting executable code directly into backend command executors.
- **Residual Risk:** Medium (inherent to generative AI).
- **Verification Method:** Unit tests verifying prompt formatting envelopes and escaping of delimiter tokens across provider adapters.

### T-04: Denial of Service (DoS) via Oversized Requests & Decompression Bombs
- **STRIDE Category:** Denial of Service
- **Attack Scenario:** Attacker sends multi-gigabyte payloads or gzip decompression bombs to exhaust backend memory and trigger microVM Out-Of-Memory (OOM) crashes.
- **Likelihood:** High | **Impact:** High | **Risk:** High
- **Mitigations:**
  1. Enforce strict Axum request body limits (`DefaultBodyLimit::max(4_500_000)` bytes = 4.5 MB).
  2. Reject requests exceeding maximum declared `Content-Length` headers before reading body streams.
  3. Enforce decompression ratio limits on compressed payloads.
- **Residual Risk:** Low.
- **Verification Method:** Integration test sending a 10 MB payload and asserting immediate HTTP 413 Payload Too Large.

### T-05: Denial of Wallet / Quota Exhaustion through Request Flooding
- **STRIDE Category:** Denial of Service
- **Attack Scenario:** Attacker floods `/api/v1/chat/completions` or image-generation endpoints to exhaust free-tier provider allocations or incur billing charges.
- **Likelihood:** High | **Impact:** High | **Risk:** High
- **Mitigations:**
  1. Enforce token bucket rate limiting per authenticated user and per IP.
  2. Require valid authentication for all AI execution endpoints.
  3. Restrict expensive operations (image/video generation) to lower rate-limit tiers than simple text queries.
  4. Implement bounded retry budgets (max 1 retry, zero retries on non-idempotent media endpoints).
- **Residual Risk:** Low.
- **Verification Method:** Load testing simulating 100 rapid requests from a single client and verifying HTTP 429 Too Many Requests response after threshold.

### T-06: Google Drive OAuth Token Theft or Abuse
- **STRIDE Category:** Information Disclosure / Elevation of Privilege
- **Attack Scenario:** Attacker intercepts Google OAuth access tokens or steals them from backend memory, accessing the user's personal Google Drive.
- **Likelihood:** Low | **Impact:** Critical | **Risk:** High
- **Mitigations:**
  1. Enforce least-privilege OAuth scope: request strictly `https://www.googleapis.com/auth/drive.appdata`. This folder is hidden from user view and cannot access personal files outside the app.
  2. Never persist Google OAuth access or refresh tokens on the backend; pass them in transit over TLS and use ephemeral memory.
  3. Validate token audience and issuer on every request.
- **Residual Risk:** Low.
- **Verification Method:** Automated test verifying that OAuth scope requests reject `drive` or `drive.readonly` and only accept `drive.appdata`.

### T-07: Cross-User Conversation Leakage
- **STRIDE Category:** Information Disclosure
- **Attack Scenario:** Memory reuse or flawed session routing causes User A to receive User B's streaming response or cached conversation history.
- **Likelihood:** Low | **Impact:** Critical | **Risk:** High
- **Mitigations:**
  1. Completely stateless request lifecycle: no shared in-memory conversation caches across sessions.
  2. Strict tenancy validation: all Drive history requests require matching the authenticated user ID in the verified token.
  3. Correlation ID tracking per request lifecycle.
- **Residual Risk:** Very Low.
- **Verification Method:** Concurrency test issuing parallel chat streams from distinct mock users and asserting zero data crossover.

### T-08: Malicious or Compromised Third-Party GitHub Repositories
- **STRIDE Category:** Tampering / Elevation of Privilege
- **Attack Scenario:** A third-party repository integrated as a provider adapter is compromised, modifies its upstream API behavior, or exfiltrates user prompts.
- **Likelihood:** Medium | **Impact:** High | **Risk:** High
- **Mitigations:**
  1. Pin all repository dependencies to explicit commit hashes or immutable release tags.
  2. Review repository license, maintainer provenance, and source code before onboarding.
  3. Restrict network egress for the adapter exclusively to documented official API hosts.
  4. Follow the strict 9-point checklist in [`PROVIDER_INTEGRATION.md`](file:///d:/Rust%20Backend/ai-backend/PROVIDER_INTEGRATION.md#L42-L51).
- **Residual Risk:** Low.
- **Verification Method:** Dependency audit and static code analysis before adapter inclusion.

### T-09: Unbounded Hanging Connections & Serverless Resource Exhaustion
- **STRIDE Category:** Denial of Service
- **Attack Scenario:** Upstream provider hangs indefinitely during streaming, causing Vercel serverless microVMs to hit the 15-second hard timeout and terminate abruptly.
- **Likelihood:** High | **Impact:** Medium | **Risk:** Medium
- **Mitigations:**
  1. Apply strict timeouts on upstream HTTP client calls (e.g. 10 seconds default).
  2. Implement client disconnect detection: cancel upstream requests immediately if client drops the SSE connection.
  3. Fail fast with standardized HTTP 504 Gateway Timeout before the hosting platform terminates the microVM.
- **Residual Risk:** Low.
- **Verification Method:** Wiremock test simulating a 30-second delay and asserting the backend cancels at 10s and returns HTTP 504.

### T-10: Software Supply Chain & Dependency Vulnerabilities
- **STRIDE Category:** Elevation of Privilege / Tampering
- **Attack Scenario:** A transitive Rust dependency in `Cargo.lock` contains a known remote code execution or memory corruption vulnerability.
- **Likelihood:** Medium | **Impact:** Critical | **Risk:** High
- **Mitigations:**
  1. Maintain a minimal dependency tree (avoid sprawling dependency trees).
  2. Enforce `cargo audit` in continuous integration to catch vulnerabilities in the RustSec advisory database.
  3. Enforce `cargo deny` to verify licenses and ban unapproved crates.
- **Residual Risk:** Low.
- **Verification Method:** Execution of `cargo audit` and `cargo deny check` in automated test pipelines.

---

## 3. Residual Risk & Limitations

No software system is entirely immune to compromise. The following residual risks are acknowledged:
1. **Zero-Day LLM Vulnerabilities:** Direct and indirect prompt injection cannot be 100% prevented by software guards alone when processing arbitrary user text.
2. **Upstream Provider Outages:** If all integrated providers in a given task category experience simultaneous outages, the backend can only return a graceful, informative error.
3. **Serverless Ephemeral Memory:** In-memory rate limiting counters reset during cold starts; malicious distributed attacks across multiple microVM instances must be complemented by edge-level platform protections.
