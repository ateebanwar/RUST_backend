# DEPLOYMENT.md — Deployment Architecture, Vercel Evaluation & Operations Guide

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Deployment Target:** Existing Vercel Account (Zero-Cost Infrastructure Objective)  
**Status:** Architecture Evaluated & Dual-Target Deployment Model Defined  
**Last Updated:** 2026-10-19

---

## 1. Technical Evaluation of Vercel for Rust Backends

### 1.1 Vercel Execution Architecture
Vercel executes backend code as serverless functions running on AWS Lambda microVMs (Amazon Linux 2023 / `provided.al2023`).

To run a Rust Axum application on Vercel:
1. **Compilation Bridge:** The Rust code is compiled using `cargo-lambda` or the official `vercel-rust` community builder into a standalone Linux binary named `bootstrap`.
2. **Framework Adapter:** Axum's `axum::Router` is wrapped by the `lambda_http` crate using `lambda_http::run(router)`. Incoming API Gateway / Vercel HTTP events are seamlessly converted into standard Tower/Hyper HTTP requests and dispatched to Axum handlers.
3. **Local Emulation:** `cargo run` executes the standard Axum HTTP server locally on `http://127.0.0.1:3000`, while `vercel dev` emulates the serverless microVM lifecycle.

### 1.2 Platform Constraints & Operational Implications

| Constraint Dimension | Vercel Serverless Limit | Operational Impact on `ai-backend` | Architectural Mitigation |
|---|---|---|---|
| **Execution Duration** | 15s (Hobby) / 60s (Pro) | Upstream AI queries exceeding 15s get killed with 504 Gateway Timeout. | Enforce strict 10s upstream HTTP client timeouts; select fast inference models. |
| **Max Payload Size** | 4.5 MB request/response | Large file uploads (e.g., big PDFs or videos) will fail at gateway. | Enforce 4.5 MB body limit; advise client-direct Drive uploads for large media. |
| **Instance Memory** | Ephemeral microVM | Memory is wiped when instances spin down; no shared memory between parallel requests. | Stateless design: circuit breakers use fast-decay heuristics; chat history in Google Drive. |
| **Cold Starts** | ~150 ms – 350 ms | Initial request after idle period incurs slight startup delay. | Minimize binary size, disable unused default crate features, strip symbols (`opt-level = "z"` / `lto = true`). |
| **Streaming / SSE** | Supported via chunked transfer | Client can stream tokens, but total stream time must complete within 15s. | Limit completion token length on serverless deployments to fit within 15s window. |

---

## 2. Hosting Architecture Comparison & Recommendation

| Dimension | Option 1: Vercel Serverless Function (Approved Target) | Option 2: Dedicated Rust Container (e.g., Fly.io / Render / VPS) | Option 3: Split Architecture |
|---|---|---|---|
| **Recurring Cost** | **$0.00 / month** (Leverages user's existing Vercel tier) | $0.00 (limited free tiers) or $3–$7 / month | Mixed |
| **Timeout Limits** | 15s hard limit (Hobby) | Unlimited / Fully configurable | 15s on API, unlimited on worker |
| **In-Memory State** | Ephemeral only | Persistent (in-memory connection pools, live circuit breakers) | Complex distributed state |
| **Operational Effort** | Zero server management (git push to deploy) | Requires Dockerfile, monitoring, and restart recovery | High (two independent deployment pipelines) |

### Approved Recommendation: Dual-Target Deployment Model
To achieve **zero recurring cost** while preserving maximum flexibility:
1. **Core Decoupling:** Implement all routing, adapters, and business logic in a shared library crate (`src/lib.rs`).
2. **Dual Entrypoints:**
   - `src/main.rs`: Compiles a standard, persistent Axum HTTP daemon for local development, automated integration testing, and optional future container deployment.
   - `api/index.rs` (or serverless bootstrap): Compiles a lightweight binary linking `lib.rs` to `lambda_http::run` for deployment to Vercel Serverless Functions.
3. **Stateless Gracefulness:** Operate circuit breakers, usage counters, and rate limiters with heuristic decay, relying on Google Drive for user persistence.
4. **Upgrade Path:** If deep reasoning or long multimodal video generation workflows consistently exceed 15 seconds, migrate the existing `src/main.rs` binary to a low-cost container service without modifying application logic.

---

## 3. Environment & Secret Configuration

The following environment variables must be configured in the Vercel project settings:

```bash
# Server Configuration
ENVIRONMENT="production"
LOG_LEVEL="info"
APP_URL="https://your-vercel-domain.vercel.app"

# Multi-Client Authentication
GOOGLE_CLIENT_ID="<your-google-oauth-client-id>.apps.googleusercontent.com"
DEVELOPER_API_KEYS="hash1,hash2" # Comma-separated constant-time hashes for IDEs

# AI Provider API Keys (Server-Side Only - Never Exposed)
GROQ_API_KEY="gsk_..."
OPENROUTER_API_KEY="sk-or-..."
ANTHROPIC_API_KEY="sk-ant-..."
OPENAI_API_KEY="sk-..."
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
