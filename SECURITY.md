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
   - User identity authenticated via Google Sign-In OIDC ID tokens.
   - The backend validates token signatures against Google's public JSON Web Key Sets (JWKS), checking `iss` (`https://accounts.google.com`), `aud` (configured Google Client ID), and expiration timestamp (`exp`).
   - Client-asserted user IDs are never trusted without cryptographic token validation.
2. **Developer & IDE Clients (Custom IDE, VS Code Extension):**
   - Authenticated via Personal Access Tokens (PATs) or backend API keys passed in the `Authorization: Bearer <pat>` or `X-API-Key: <key>` headers.
   - Keys must be hashed using constant-time comparison algorithms (e.g. `subtle::ConstantTimeEq`) to prevent timing side-channel attacks.
3. **Role & Resource Authorization:**
   - Every protected API endpoint verifies resource ownership. A user can only access their own conversation synchronization streams.

---

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
