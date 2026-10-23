# DEPLOYMENT.md — Deployment Architecture, Vercel Evaluation & Operations Guide

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Deployment Target:** Existing Vercel Account (Zero-Cost Infrastructure Objective)  
**Status:** Architecture Evaluated & Dual-Target Deployment Model Defined  
**Last Updated:** 2026-10-19

---

## 1. Technical Evaluation of Vercel for Rust Backends

### 1.1 Vercel Execution Architecture: Fluid Compute Containers vs Serverless Functions
Vercel supports running backend applications under two distinct execution paradigms:

1. **Fluid Compute Containers (`Dockerfile.vercel` - Approved Primary Target):**
   - Vercel automatically detects `Dockerfile.vercel` at repository root, compiles the OCI-compliant container image via multi-stage Docker build, and deploys it on Vercel Functions powered by **Fluid compute**.
   - **Zero Code Changes:** Runs the unmodified native Axum HTTP daemon (`src/main.rs`) listening on the port provided by `$PORT` (which `src/config.rs` natively reads).
   - **Active CPU Billing:** Billed only for CPU cycles while actively processing HTTP requests (idle I/O waiting is not billed).
   - **Lifecycle:** Scales up on demand and scales to zero when idle. Handles `SIGTERM` gracefully via Axum's built-in graceful shutdown.

2. **Native Serverless Functions (`vercel_runtime` / `lambda_http` Adapter):**
   - Requires adding `vercel_runtime` or `lambda_http` to `Cargo.toml` and maintaining an `api/index.rs` entry point.
   - Converts AWS Lambda JSON RPC events into Tower/Hyper requests.
   - Suffers from cold start latency (~150ms–350ms) and requires maintaining dual entry points.

### 1.2 Platform Constraints & Operational Implications (Fluid Compute)

| Constraint Dimension | Vercel Fluid Compute Limit | Operational Impact on `ai-backend` | Architectural Mitigation |
|---|---|---|---|
| **Execution Duration** | 300s (Hobby & Pro default) / up to 800s (Pro max) | Requests exceeding timeout get terminated with `504 Gateway Timeout`. | Fast inference (Groq) takes 1–4s; deep reasoning (DeepSeek-R1) takes 20–60s; both finish well under 300s limit. |
| **Max Payload Size** | 4.5 MB request/response | Payloads exceeding 4.5 MB are rejected at the edge gateway. | Enforced by Axum `DefaultBodyLimit::max(4_500_000)` in `src/lib.rs`. |
| **Instance Memory** | Ephemeral microVM (scales to zero) | Memory is wiped when instances scale to zero; no shared memory between parallel container instances. | Stateless architecture: Chat history and recovery blobs persist directly to Google Drive via AES-256-GCM. |
| **Cold Starts** | ~200 ms – 500 ms (microVM boot) | Initial request after idle period incurs minor startup latency. | Multi-stage slim Docker image (`debian:bookworm-slim`), stripped binary, zero heavy native dynamic libs. |
| **SSE Streaming** | Supported via HTTP chunked transfer | Client receives streaming tokens in real time. | Native Axum `Sse` response stream with `KeepAlive` heartbeat and atomic drop-guard usage finalization. |
| **Background Tasks** | Frozen when container scales to zero | Scheduled hourly maintenance (`start_retention_maintenance`) cannot tick when idle. | Invariant cleanup executes on startup in `AppState::new`. For continuous 24/7 workers, use persistent container. |

---

## 2. Hosting Architecture Comparison & Recommendation

| Dimension | Option 1: Vercel Container via `Dockerfile.vercel` (Primary Target) | Option 2: Dedicated Rust Container (Fly.io / Render / Hetzner VPS) | Option 3: Serverless Function (`vercel_runtime`) |
|---|---|---|---|---|
| **Recurring Cost** | **$0.00 / month** (Leverages user's existing Vercel tier) | $0.00 (limited free tiers) or $3–$7 / month | $0.00 / month |
| **Timeout Limits** | 300s (Hobby) / up to 800s (Pro) | Unlimited / Fully configurable | 15s (legacy Hobby) / 300s (Fluid) |
| **In-Memory State** | Ephemeral (resets on scale-to-zero) | Persistent 24/7 (in-memory rate limiter & token buckets) | Ephemeral per invocation |
| **Background Maintenance** | Runs during active traffic; frozen when idle | Runs 24/7 uninterrupted on hourly ticker | Severely constrained (frozen on idle) |
| **Code Modifications** | **0 lines changed** (native Axum server) | **0 lines changed** (native Axum server) | Requires `vercel_runtime` & `api/index.rs` adapter |

### Approved Recommendation: Container-First Dual Target Model
1. **Primary Target (Vercel):** Deploy via `Dockerfile.vercel`. Provides zero-cost hosting on Vercel Fluid Compute with zero Rust code changes, preserving all frontend contracts, SSE streaming, authentication, and encrypted Google Drive persistence.
2. **Upgrade Path (Fly.io / Render / VPS):** The exact same Docker container can be deployed to any container host with zero modifications if 24/7 uninterrupted background maintenance loops or persistent in-memory rate-limiter buckets are needed.

---

## 3. Environment & Secret Configuration

Configure the following environment variables in the Vercel Project Settings (or `.env.production`):

```bash
# Server & Runtime Configuration
HOST="0.0.0.0"
PORT="8080"
APP_ENV="production"
DEPLOYMENT_MODE="daemon"
LOG_FORMAT="json"
ALLOWED_HOSTS="your-project.vercel.app,api.yourdomain.com"
TRUSTED_PROXIES="127.0.0.1,::1"

# Client Authentication & Multi-Client Security
API_KEY="your_production_backend_api_key_here"
GOOGLE_CLIENT_ID="your_google_client_id.apps.googleusercontent.com"
AUTH_REQUIRED="true"

# Google Drive Encrypted Sync & Persistence
GOOGLE_DRIVE_SYNC_ENABLED="true"
DRIVE_ENCRYPTION_KEY="your_64_hex_char_master_key_here"
GOOGLE_DRIVE_FOLDER_NAME="ai_backend_app_data"
GOOGLE_DRIVE_TIMEOUT_SECS="30"
GOOGLE_DRIVE_MAX_RETRIES="3"

# Upstream AI Provider API Keys (Server-Side Only - Never Exposed)
GROQ_API_KEY="gsk_..."
OPENROUTER_API_KEY="sk-or-v1-..."
UPSTREAM_TIMEOUT_SECS="10"

# Abuse Prevention & Rate Limiting
RATE_LIMIT_ENABLED="true"
RATE_LIMIT_IP_BURST="100"
RATE_LIMIT_IP_RATE="20.0"
MAX_CONCURRENT_REQUESTS="200"
MAX_CONCURRENT_STREAMS="50"
SSRF_PROTECTION_ENABLED="true"
```

---

## 4. Deployment Verification & Rollback Procedures

### 4.1 Post-Deployment Smoke Tests
Immediately after deploying to Vercel, execute:
```bash
# 1. Verify liveness probe returns HTTP 200
curl -f https://<deployment-url>/api/v1/health

# 2. Verify readiness probe reports active providers
curl -f https://<deployment-url>/api/v1/ready

# 3. Verify unauthorized requests are rejected
curl -I https://<deployment-url>/api/v1/models
# Expected output: HTTP/2 401
```

### 4.2 Emergency Rollback
If a deployment exhibits regressions or runtime crashes:
1. Navigate to the Vercel Dashboard → **Deployments**.
2. Locate the previous known-good deployment.
3. Click the menu icon and select **Instant Rollback**.
4. Traffic is immediately shifted back to the previous deployment within seconds.
