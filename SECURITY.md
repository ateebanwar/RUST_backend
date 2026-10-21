# SECURITY.md — Baseline Security Policies & Operating Controls

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Status:** Mandatory Operational Security Baseline  
**Last Updated:** 2026-10-19

---

## 1. Secret Management & Credential Isolation

1. **Zero Client Secrets:** Provider API keys and server-side credentials must NEVER be compiled into client binaries (Flutter APKs, Custom IDE executables, or VS Code extensions), transmitted to frontends, or checked into version control.
2. **Environment Variable Injection:** All provider API keys and signing secrets must be injected at runtime via environment variables (or Vercel Secret Management).
3. **Memory Isolation via `SecretString`:** All secret values loaded into memory must be wrapped in custom types (such as `secrecy::SecretString`) that zeroize memory upon drop and override `Debug` and `Display` to print `[REDACTED]`.
4. **Automated Secret Scanning:** Automated git pre-commit hooks and CI pipelines must scan for accidental credential commits using tools like `trufflehog` or `gitleaks`.

---

## 2. Multi-Client Authentication & Authorization

1. **Mobile Users (Flutter):**
    - User identity authenticated via Google Sign-In OIDC ID tokens passed in `Authorization: Bearer <google_id_token>`.
    - The backend cryptographically validates RS256 token signatures against Google's public JSON Web Key Sets (JWKS) via `jsonwebtoken`.
    - Strictly checks `iss` (`https://accounts.google.com` or `accounts.google.com`), `aud` (configured Google Client ID(s)), expiration timestamp (`exp` with 60-second RFC 7519 leeway), issued-at (`iat`), not-before (`nbf`), authorized party (`azp`), and non-empty subject (`sub`).
    - JWKS keys are cached in memory with a 1-hour TTL, bounded 5-second fetch timeout (`HttpJwksProvider`), strict 256 KB response body limit, and 10-second refresh throttling on unknown `kid` to prevent upstream cache-miss flooding. Deterministic offline testing is guaranteed via `MockJwksProvider` without live Google network access.
    - Client-asserted user IDs are never trusted without cryptographic token validation.
 2. **Developer & IDE Clients (Custom IDE, VS Code Extension, CLI):**
    - Authenticated via Personal Access Tokens (PATs) (`pat_<64_hex_chars>`) or configured backend API keys passed in `Authorization: Bearer <pat>` or `X-API-Key: <key>`.
    - Keys are stored in memory as SHA-256 hashes (`ring::digest`) and verified using constant-time comparison (`subtle::ConstantTimeEq`), eliminating length- and character-timing side channels.
    - `ConfiguredCredential` implements zero-leak `Debug` formatting redacting key hashes as `[REDACTED]`.
 3. **Identity Propagation & Route Scope Authorization:**
    - Every authenticated request injects a strongly-typed `AuthPrincipal` into Axum request extensions, carrying `subject`, `auth_type` (`GoogleOidc`, `PersonalAccessToken`, `ApiKey`, `DevelopmentBypass`), `client_type` (`Flutter`, `CustomIde`, `VscodeExtension`, `ApiClient`), `email`, `roles` (`User`, `Admin`, `Service`), and `scopes`.
    - Ingress middleware verifies that the principal possesses the documented scope for the target endpoint: `models:read` for model & provider discovery, `routing:read` for routing preview, `chat` for chat completions and streaming, and `usage:read` for usage and quotas. Requests lacking the required scope are denied with HTTP 403 Forbidden.
    - Caller `X-Client-Type` headers are audit labels only and strictly never grant roles, scopes, or privileges.
 4. **Query Parameter Credential Prohibition:**
    - Credentials must NEVER be accepted through URL query parameters (e.g. `?api_key=...`, `?access_token=...`, `?api-key=...`, `?bearer=...`).
    - URL query strings are automatically percent-decoded to prevent evasion attacks using percent-encoded parameters (e.g. `?api%5fkey=...` or `%61%70%69...`).
    - Inbound requests containing credentials in query parameters are unconditionally rejected with HTTP 401 Unauthorized, instructing clients to use standard request headers. Query credentials never grant development bypass or fall back to other authentication methods.
 5. **Strict Precedence & Zero Fallback:**
    - Incoming 3-segment JWTs are routed strictly to the Google OIDC validator.
    - OIDC verification strictly validates signature, `iss`, `aud`, `exp`, `nbf`, `iat`, and `azp` (Authorized Party presenter). If `azp` is present, it MUST match one of the configured Google client IDs.
    - JWKS response downloads are bounded by a 256 KB streaming body limit and throttled by a 10-second refresh cooldown on unknown `kid` misses under an atomic write lock.
    - Non-JWT tokens and `X-API-Key` headers are routed strictly to the PAT/API Key validator.
    - An invalid credential never receives weaker fallback authentication or developer bypass.
 6. **Fail-Closed Production & External Bind Security:**
    - In production mode (`APP_ENV=production`), application startup fails closed with `ConfigError::InvalidValue` if no valid `API_KEY`, `API_KEYS`, or `GOOGLE_CLIENT_ID` is present, or if `GOOGLE_JWKS_URL` does not use HTTPS, or if `AUTH_REQUIRED` is configured to `false`.
    - Unauthenticated development bypass is strictly restricted to loopback network interfaces (`127.0.0.1`, `localhost`, `::1`). Binding to any non-loopback network interface (`HOST=0.0.0.0` or public IP) unconditionally enforces authentication across ALL protected routes.

-----

## 3. Request Hardening & Abuse Prevention

1. **Strict Request Body Size Limits:**
   - Inbound request bodies are hard-capped at 4.5 MB (`DefaultBodyLimit::max(4_500_000)` bytes), matching serverless execution constraints and preventing memory exhaustion.
2. **Execution Timeouts:**
   - All inbound requests are governed by strict timeouts.
   - All external HTTP calls to AI providers are bounded by a maximum 10-second client timeout, preventing hanging microVM instances.
3. **Token Bucket Rate Limiting:**
   - Rate limiting is enforced per authenticated user identity and per client IP address to prevent denial of wallet and quota depletion.

---

## 4. SSRF Defense & Egress Allowlisting

To prevent Server-Side Request Forgery (SSRF) and cloud metadata exfiltration:
1. **Dynamic URL Prohibition:** Provider endpoint URLs must be defined strictly in server-side configuration, never accepted from user-submitted request payloads.
2. **Address Validation:** Any outbound HTTP client must validate resolved IP addresses and immediately reject:
   - IPv4 Private Addresses (RFC 1918: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
   - Loopback Addresses (`127.0.0.0/8`, `::1`)
   - Link-Local & Cloud Metadata (`169.254.0.0/16`)
   - Broadcast and multicast addresses.
3. **Strict Protocol Allowlist:** Outbound connections must strictly use `https://`. Insecure schemes (`http://`, `file://`, `ftp://`, `gopher://`) are banned.

---

## 5. Google Drive Least Privilege & Token Security

1. **Restricted OAuth Scope:**
   - The backend strictly requires the narrowest possible Drive scope: `https://www.googleapis.com/auth/drive.appdata`.
   - This scope restricts the application to its own private, hidden Application Data folder. It CANNOT view, list, modify, or delete any of the user's personal files, photos, or documents.
2. **Ephemeral Transit Only:**
   - Google Drive access tokens are passed in request headers (`X-Drive-Token`) and processed entirely in ephemeral memory.
   - The backend NEVER stores Google OAuth refresh tokens or access tokens on disk or in a permanent database.
3. **Safe Logout Semantics:**
   - Logging out of a client application terminates the local session only. Remote Google Drive chat backups must NEVER be deleted automatically upon logout.

---

## 6. Safe Error Handling & Telemetry Sanitization

1. **Sanitized Error Envelopes:**
   - Errors returned to clients must use standardized codes (e.g. `UPSTREAM_FAILURE`, `RATE_LIMITED`) and safe descriptions.
   - Raw stack traces, internal IP addresses, upstream vendor headers, and database connection strings must NEVER be included in response payloads.
2. **Log Redaction:**
   - Structured JSON logging must automatically redact `Authorization`, `X-API-Key`, `X-Drive-Token`, and cookie headers.
   - Full user prompt text and generated completions must NOT be logged at `INFO` or `WARN` levels in production.

---

## 7. Supply Chain & Dependency Management

1. **Minimal Dependencies:** Prioritize the standard library and well-audited, actively maintained crates.
2. **Automated Vulnerability Auditing:** CI pipelines must run `cargo audit` against the RustSec Advisory Database before merging code or publishing builds.
3. **License & Dependency Policy:** Enforce `cargo deny` to prevent the introduction of copyleft licenses (GPL) or banned/unmaintained crates.

---

## 8. Incident Response & Credential Rotation

If an upstream provider key or signing secret is suspected of being compromised:
1. **Immediate Revocation:** Rotate the compromised credential in the upstream provider's console immediately.
2. **Environment Update:** Update the secret in the Vercel project settings (or server environment) and trigger a redeployment.
3. **Audit Log Inspection:** Review provider usage dashboards for abnormal token spikes or unauthorized completions.
4. **Security Advisory:** Document the event, root cause, and remediation in [`CHANGELOG.md`](file:///d:/Rust%20Backend/ai-backend/CHANGELOG.md).

---

## 9. Network Host & Reverse-Proxy Security Hardening

1. **Loopback Bind Default:** Local daemon development binds strictly to `127.0.0.1` by default. Binding to external interfaces (`0.0.0.0`) requires deliberate configuration via the `HOST` environment variable.
2. **Trusted Reverse-Proxy Allowlisting:**
   - Inbound `X-Forwarded-For`, `Forwarded`, `X-Forwarded-Host`, and `X-Forwarded-Proto` headers are untrusted and discarded by default.
   - Forwarded headers are trusted only when the immediate peer connection originates from an explicitly configured CIDR or IP address defined in `TRUSTED_PROXIES` (e.g. `127.0.0.1/32`, `10.0.0.0/8`, `2001:db8::/32`).
3. **HTTP Host-Header Poisoning Defense:**
   - When `ALLOWED_HOSTS` is configured, incoming `Host` headers must strictly match the allowlist. Unmatched host headers are rejected with HTTP 403 Forbidden.
4. **Ingress mTLS Protocol Boundary:**
   - Mutual TLS (mTLS) client certificate verification terminates at the reverse-proxy or API ingress layer (e.g. Cloudflare, Nginx, Envoy). The Rust application does not process raw TLS client certificates directly.

---

## 10. Confidential Files & Version Control Hygiene

1. **Strict Git Exclusion:** `.env`, `.env.local`, `data/*.db*`, `target/`, and private keys are strictly excluded from version control via `.gitignore`.
2. **Tracked Secret Prevention:** Local SQLite databases and compiled binaries must never be added to git index.
3. **Safe Example Templates:** Only sanitized configuration templates containing non-secret placeholder variables (such as `.env.example`) are maintained in version control. Real credentials must never be committed.
