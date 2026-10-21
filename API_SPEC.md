# API_SPEC.md — REST API & Streaming Protocol Specification

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)  
**Base URL:** `/api/v1`  
**Status:** Finalized Phase 2 API Contract Specification  
**Last Updated:** 2026-10-19

---

## 1. Global Conventions & Standards

### 1.1 Authentication & Client Identification Headers
All endpoints (except public probes `/api/v1/health` and `/api/v1/ready`) require authentication via standard HTTP headers:

| Header Name | Required? | Accepted Values / Format | Description |
|---|---|---|---|
| `Authorization` | Yes* | `Bearer <google_id_token>` OR `Bearer <pat>` OR `Bearer <api_key>` | Primary authentication header (*either valid Bearer token or `X-API-Key` required). 3-segment JWTs are validated via Google OIDC RS256 JWKS; other tokens are validated as PATs or API keys via constant-time SHA-256 hash comparison. |
| `X-API-Key` | Yes* | `<min-8-char-key>` (e.g. 64-hex key or configured backend key) | Dedicated API key header for developer IDE, CLI, and automated service consumers. Evaluated in constant time. |
| `X-Client-Type` | Optional | `flutter` \| `custom-ide` \| `vscode-extension` \| `api-client` | Client identifier header classifying caller. Injected into `AuthPrincipal.client_type` for audit logging and quota policy. Defaults to `api-client`. |
| `X-Request-ID` | Optional | Alphanumeric, hyphen, and underscore string up to 128 chars | Tracing correlation ID. If omitted or malformed, backend generates a unique `req_<hex>` ID and propagates it in all response headers and error envelopes. |

#### Route Protection Matrix

| Route Path | Method | Authentication Policy | Minimum Role / Scope | Description |
|---|---|---|---|---|
| `/api/v1/health` | `GET` | **Public** (Unauthenticated) | None | Container liveness and process health probe |
| `/api/v1/ready` | `GET` | **Public** (Unauthenticated) | None | Upstream model discovery & inference readiness probe |
| `/api/v1/models` | `GET` | **Authenticated** | Scope `models:read` | Query catalog of registered AI models |
| `/api/v1/models/{model_id}` | `GET` | **Authenticated** | Scope `models:read` | Lookup model metadata and capability spec |
| `/api/v1/providers` | `GET` | **Authenticated** | Scope `models:read` | List active provider adapters & operational status |
| `/api/v1/routing/preview` | `POST` | **Authenticated** | Scope `routing:read` | Preview task classification and candidate scoring |
| `/api/v1/chat/completions` | `POST` | **Authenticated** | Scope `chat` | Execute buffered multi-turn chat completion |
| `/api/v1/chat/stream` | `POST` | **Authenticated** | Scope `chat` | Execute real-time SSE token streaming chat |
| `/api/v1/usage/summary` | `GET` | **Authenticated** | Scope `usage:read` | Aggregated usage & cost accounting metrics |
| `/api/v1/usage/providers` | `GET` | **Authenticated** | Scope `usage:read` | Provider-level token and request metrics |
| `/api/v1/usage/models` | `GET` | **Authenticated** | Scope `usage:read` | Model-level token and request metrics |
| `/api/v1/quotas` | `GET` | **Authenticated** | Scope `usage:read` | Active rate-limit headroom and quota telemetry |

> [!IMPORTANT]
> **Strict Precedence & Zero Fallback Rule:** If an incoming request provides credentials (e.g. an expired Google ID token or invalid API key), it is rejected with HTTP 401 Unauthorized immediately. Invalid credentials never fall back to another authentication mechanism or developer bypass.

### 1.2 Standardized JSON Data Types & Formatting Rules
- **Timestamps:** ISO 8601 / RFC 3339 UTC strings with trailing `Z` (e.g. `2026-10-19T17:00:00Z`).
- **Nullability:** Omitted fields or explicit `null` represent absent/unknown data. Unknown provider quotas must be serialized as `null`, never fabricated as `0`.
- **String Limits:** Prompt strings are capped at 100,000 characters per message; model IDs at 128 characters; titles at 256 characters.
- **Maximum Request Body:** Enforced hard ceiling of 4,500,000 bytes (4.5 MB).

### 1.3 Standardized Error Envelope
All error responses (HTTP 4xx and 5xx) must strictly conform to the canonical error envelope:

```json
{
  "error": {
    "code": "RATE_LIMITED",
    "message": "Upstream provider rate limit exceeded. Fallback model engaged.",
    "retryable": true,
    "request_id": "req_01h7x9k2p8m3...",
    "details": {
      "provider_id": "groq-api",
      "retry_after_seconds": 5
    }
  }
}
```

#### Canonical Error Code Mapping

| HTTP Status | Error Code | Description / Trigger Scenario | Retryable? |
|---|---|---|---|
| **400** | `INVALID_REQUEST` | Malformed JSON, failed validation, missing required fields. | No |
| **401** | `UNAUTHORIZED` | Missing, expired, or cryptographically invalid auth token. | No |
| **403** | `FORBIDDEN` | Valid token, but insufficient permissions for requested resource. | No |
| **404** | `NOT_FOUND` | Specified model ID or conversation ID does not exist. | No |
| **413** | `PAYLOAD_TOO_LARGE` | Inbound payload exceeds 4.5 MB body limit. | No |
| **422** | `UNSUPPORTED_MODALITY` | Requested task requires modality (e.g. vision) unsupported by model. | No |
| **429** | `RATE_LIMITED` | Inbound user rate limit or upstream provider quota exceeded. | Yes |
| **502** | `UPSTREAM_FAILURE` | Upstream AI provider returned 5xx or unparsable response. | Yes |
| **504** | `GATEWAY_TIMEOUT` | Upstream provider request exceeded 10-second deadline. | Yes |
| **500** | `INTERNAL_ERROR` | Internal server or unexpected unhandled condition. | No |

---

## 2. Health & System Discovery Endpoints

### 2.1 Liveness Probe
- **Method & Path:** `GET /api/v1/health`
- **Authentication:** None (Public)
- **Response Headers:** `Content-Type: application/json`
- **Status Code:** `200 OK`
- **Response Schema:**
```json
{
  "status": "healthy",
  "version": "0.1.0",
  "timestamp": "2026-10-19T17:00:00Z"
}
```

### 2.2 Readiness Probe
- **Method & Path:** `GET /api/v1/ready`
- **Authentication:** None (Public)
- **Status Code:** `200 OK` (if at least one provider is ready) or `503 Service Unavailable`
- **Response Schema (Operational / Ready):**
```json
{
  "status": "ready",
  "active_providers": 3,
  "registered_models": 12,
  "deployment_mode": "serverless"
}
```
- **Response Schema (Unready / 503):**
```json
{
  "status": "not_ready",
  "active_providers": 0,
  "registered_models": 0,
  "deployment_mode": "daemon"
}
```
- **Semantics:**
  - `active_providers` counts only registered upstream adapters that are enabled and operational (`status == "operational"`).
  - `registered_models` counts only models in the registry that are `available: true` and belong to an active, operational provider.
  - Reference catalog models and static provider profiles do not count towards operational readiness.

---

## 3. Model Registry & Discovery Endpoints

### 3.1 List Configured Models
- **Method & Path:** `GET /api/v1/models`
- **Authentication:** Required
- **Query Parameters:**
  - `modality` (optional, string): Filter by modality (`text`, `code`, `image`, `video`, `document`).
  - `speed_tier` (optional, string): Filter by speed tier (`fast`, `balanced`, `advanced`).
  - `provider` (optional, string): Filter by exact provider ID.
- **Status Code:** `200 OK`
- **Response Schema:**
```json
{
  "models": [
    {
      "model_id": "fast-chat-v1",
      "display_name": "Llama 3.3 70B Instruct (Groq)",
      "provider_id": "groq-api",
      "supported_modalities": ["text", "code"],
      "context_window": 131072,
      "max_output_tokens": 8192,
      "streaming_supported": true,
      "tool_calling_supported": true,
      "speed_tier": "fast",
      "cost_tier": "free-tier",
      "available": true,
      "capabilities": ["simple_chat", "fast_reasoning", "coding"],
      "last_health_check": "2026-10-19T16:55:00Z"
    }
  ]
}
```

### 3.2 Get Model by ID
- **Method & Path:** `GET /api/v1/models/{model_id}`
- **Authentication:** Required
- **Path Parameters:**
  - `model_id` (required, string): Alphanumeric identifier with hyphens (e.g. `fast-chat-v1`).
- **Success Status:** `200 OK`
- **Response Schema:** Single model object matching the structure in Section 3.1.
- **Error Status:** `404 Not Found` if `model_id` does not exist in registry.

### 3.3 List Providers
- **Method & Path:** `GET /api/v1/providers`
- **Authentication:** Required
- **Status Code:** `200 OK`
- **Response Schema:**
```json
{
  "providers": [
    {
      "provider_id": "groq-api",
      "display_name": "Groq Cloud Fast Inference",
      "status": "operational",
      "enabled": true,
      "models_count": 4,
      "rate_limit_tier": "free-tier-standard"
    }
  ]
}
```

---

## 4. Routing Preview Endpoint

### 4.1 Preview Routing Decision (Dry Run)
- **Method & Path:** `POST /api/v1/routing/preview`
- **Authentication:** Required
- **Description:** Previews task classification and model selection without sending requests to upstream AI providers.
- **Request Body Schema:**
```json
{
  "prompt": "Write a Rust function to parse an ELF binary header.",
  "requested_modality": "code",
  "routing_mode": "auto",
  "preferred_model_id": null
}
```
- **Validation Rules:**
  - `prompt`: Required, string, 1 to 100,000 characters.
  - `requested_modality`: Optional, enum (`text`, `code`, `image`, `video`, `document`). Defaults to `text`.
  - `routing_mode`: Optional, enum (`auto`, `manual`). Defaults to `auto`.
  - `preferred_model_id`: Optional, string, valid model ID if `routing_mode` is `manual`.
- **Response Schema (200 OK):**
```json
{
  "detected_task": "coding_and_debugging",
  "estimated_complexity": "high",
  "selected_model_id": "deep-reasoner-v1",
  "provider_id": "openrouter-api",
  "selection_reason": "Task identified as complex systems programming requiring deep reasoning capability.",
  "fallback_model_ids": ["fast-chat-v1"]
}
```

---

## 5. Chat Completion Endpoints

### 5.1 Synchronous / Buffered Completion
- **Method & Path:** `POST /api/v1/chat/completions`
- **Authentication:** Required
- **Request Headers:** `Content-Type: application/json`
- **Request Body Schema:**
```json
{
  "messages": [
    {
      "role": "system",
      "content": "You are an expert Rust backend architect."
    },
    {
      "role": "user",
      "content": "Explain how Axum handles Tower middleware layers."
    }
  ],
  "routing_mode": "auto",
  "requested_model_id": null,
  "temperature": 0.7,
  "max_tokens": 2048,
  "stream": false
}
```
- **Validation Rules:**
  - `messages`: Required, array of 1 to 50 message objects.
  - `messages[].role`: Required, enum (`system`, `user`, `assistant`).
  - `messages[].content`: Required, string, 1 to 100,000 characters.
  - `temperature`: Optional, float between `0.0` and `2.0`. Default: `0.7`.
  - `max_tokens`: Optional, integer between `1` and `32768`.
  - `stream`: Boolean, must be `false` for buffered endpoint.
- **Response Schema (200 OK):**
```json
{
  "id": "chatcmpl_01h7x9m1p2k4...",
  "model_used": "fast-chat-v1",
  "provider_used": "groq-api",
  "is_fallback": false,
  "routing_reason": "Standard architectural query matched fast balanced model.",
  "message": {
    "role": "assistant",
    "content": "Axum is built on top of Tower services and Hyper..."
  },
  "usage": {
    "prompt_tokens": 28,
    "completion_tokens": 342,
    "total_tokens": 370,
    "source": "provider_reported"
  }
}
```

---

## 6. Server-Sent Events (SSE) Streaming Protocol

### 6.1 Stream Endpoint Specification
- **Method & Path:** `POST /api/v1/chat/stream`
- **Authentication:** Required
- **Request Headers:**
  - `Accept: text/event-stream`
  - `Content-Type: application/json`
- **Response Headers:**
  - `Content-Type: text/event-stream; charset=utf-8`
  - `Cache-Control: no-cache, no-transform`
  - `Connection: keep-alive`
  - `X-Accel-Buffering: no`
- **Request Body Schema:** Identical to Section 5.1 with `"stream": true`.

### 6.2 Wire Event Framing & Schemas
Each event follows standard SSE framing: `event: <event_name>\ndata: <json_payload>\n\n`.

#### 1. Routing Metadata Event (`event: routing`)
Emitted immediately after model selection completes:
```text
event: routing
data: {"model_used":"fast-chat-v1","provider_used":"groq-api","is_fallback":false,"task_category":"coding"}

```

#### 2. Incremental Token Chunk Event (`event: chunk`)
Emitted for each content delta received from the upstream provider:
```text
event: chunk
data: {"delta":"Axum"}

event: chunk
data: {"delta":" handles"}

```

#### 3. Heartbeat Comment Frame (`: keep-alive`)
Emitted every 15 seconds if upstream generation experiences latency pauses, keeping edge proxies alive:
```text
: keep-alive

```

#### 4. Usage Summary Event (`event: usage`)
Emitted upon stream completion containing token counts:
```text
event: usage
data: {"prompt_tokens":28,"completion_tokens":142,"total_tokens":170,"source":"provider_reported"}

```

#### 5. Mid-Stream Error Event (`event: error`)
Emitted if upstream connection breaks mid-stream after HTTP headers were already sent:
```text
event: error
data: {"code":"UPSTREAM_FAILURE","message":"Upstream provider dropped connection mid-stream.","retryable":true}

```

#### 6. Completion Event (`event: done`)
Terminal event signaling clean end of stream:
```text
event: done
data: {"status":"completed"}

```

### 6.3 Disconnect & Backpressure Handling
- **TCP Disconnect Detection:** The backend monitors client socket read/write state. If the client disconnects before `event: done`, Tokio cancels the active upstream request future immediately.
- **Resource Cleanup:** Upstream HTTP connections are dropped to prevent orphaned GPU token burning.

---

## 7. Google Drive Chat History Sync Endpoints

### 7.1 Incremental Synchronization
- **Method & Path:** `POST /api/v1/drive/sync`
- **Authentication:** Required (`Authorization: Bearer <google_id_token>`)
- **Required Header:** `X-Drive-Token: <oauth_access_token_with_drive_appdata_scope>`
- **Request Body Schema:**
```json
{
  "client_last_sync_timestamp": "2026-10-19T16:00:00Z",
  "local_updates": [
    {
      "chat_id": "c_9f83a1b2...",
      "title": "Rust Backend Architecture",
      "updated_at": "2026-10-19T16:45:00Z",
      "version": 3,
      "messages": [
        {
          "id": "m_01h7x9...",
          "role": "user",
          "content": "How does Drive sync handle conflict resolution?",
          "timestamp": "2026-10-19T16:45:00Z"
        }
      ]
    }
  ]
}
```
- **Response Schema (200 OK):**
```json
{
  "sync_timestamp": "2026-10-19T17:00:00Z",
  "remote_updates": [],
  "conflicts_resolved": [
    {
      "chat_id": "c_9f83a1b2...",
      "resolution": "client_timestamp_won"
    }
  ]
}
```

---

## 8. Usage & Quota Status Endpoints

### 8.1 System Usage
- **Method & Path:** `GET /api/v1/usage`
- **Authentication:** Required
- **Query Parameters:**
  - `from` (optional, ISO timestamp): Start window.
  - `to` (optional, ISO timestamp): End window.
- **Response Schema (200 OK):**
```json
{
  "total_requests": 142,
  "total_tokens": 68420,
  "usage_by_provider": [
    {
      "provider_id": "groq-api",
      "requests": 98,
      "prompt_tokens": 12400,
      "completion_tokens": 36100,
      "is_estimated": false
    }
  ]
}
```

### 8.2 Provider Quotas & Usage Summary
- **Method & Path:** `GET /api/v1/quotas`
- **Authentication:** Required
- **Response Schema (200 OK):**
```json
{
  "providers": [
    {
      "provider_id": "groq-api",
      "requests_today": 42,
      "tokens_today": 18450,
      "rate_limit_status": "healthy",
      "quota_reported": null,
      "reset_time": null,
      "quota_status": "unknown_provider_reported"
    }
  ]
}
```

---

## 9. API Versioning, Deprecation & Backward Compatibility Policy

1. **Path-Based Versioning:** The API version is embedded in the URL path prefix (`/api/v1`).
2. **Backward-Compatible (Non-Breaking) Changes:**
   - Adding new endpoints.
   - Adding optional fields to request bodies.
   - Adding new fields to response JSON envelopes.
   - Adding new event types to SSE streams (clients must ignore unhandled event types).
3. **Breaking Changes (Requiring `/api/v2`):**
   - Removing or renaming endpoints.
   - Removing or renaming existing fields in request or response schemas.
   - Modifying data types or changing an optional field to required.
4. **Deprecation Headers:** When an endpoint is marked for deprecation:
   - `Deprecation: @<unix_timestamp>` (Date deprecation was announced).
   - `Sunset: <http_date>` (Date endpoint will be removed, minimum 90-day grace period).
