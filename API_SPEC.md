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
| `/api/v1/drive/status` | `GET` | **Authenticated** | Scope `drive` | Current Google Drive synchronization status |
| `/api/v1/drive/config` | `GET`, `POST` | **Authenticated** | Scope `drive` | Get or toggle Drive synchronization enabled status |
| `/api/v1/drive/sync` | `POST` | **Authenticated** | Scope `drive` | Trigger bidirectional synchronization with Google Drive |
| `/api/v1/drive/conversations` | `GET`, `POST` | **Authenticated** | Scope `drive` | List or create local conversations |
| `/api/v1/drive/conversations/{id}` | `GET`, `PUT`, `DELETE` | **Authenticated** | Scope `drive` | Retrieve, update, or soft-delete conversation |
| `/api/v1/drive/library` | `GET`, `POST` | **Authenticated** | Scope `drive` | List or upload Library items (files, documents, media) |
| `/api/v1/drive/library/{id}` | `GET`, `DELETE` | **Authenticated** | Scope `drive` | Retrieve metadata or delete Library item |
| `/api/v1/drive/library/{id}/content` | `GET` | **Authenticated** | Scope `drive` | Download Library item raw binary payload |
| `/api/v1/drive/restore` | `POST` | **Authenticated** | Scope `drive` | Full restore/bootstrap from Google Drive appDataFolder |
| `/api/v1/drive/logout` | `POST` | **Authenticated** | Scope `drive` | Teardown local session (preserves remote Drive data) |

> [!IMPORTANT]
> **Strict Precedence & Zero Fallback Rule:** If an incoming request provides credentials (e.g. an expired Google ID token or invalid API key), it is rejected with HTTP 401 Unauthorized immediately. Invalid credentials never fall back to another authentication mechanism or developer bypass.
>
> **Duplicate & Conflicting Headers Proscription:** Multiple `Authorization` or `X-API-Key` headers are strictly prohibited and return HTTP 401 Unauthorized. Supplying conflicting simultaneous authentication schemes (both `Authorization` and `X-API-Key` provided) is rejected with HTTP 401 Unauthorized. Credential headers must be valid UTF-8 and are subject to an enforced length ceiling of 8,192 bytes (8 KiB) to prevent memory exhaustion and DoS.
>
> **Query Parameter Credential Prohibition:** Credentials must NEVER be transmitted in URL query parameters (`?api_key=...`, `?access_token=...`, `?api-key=...`, `?bearer=...`, etc.). Query strings are automatically percent-decoded to prevent evasion attacks; inbound requests containing credentials in query parameters are strictly rejected with HTTP 401 Unauthorized.
>
> **Route Scope Authorization:** Authenticated principals must possess the required scope documented in the Route Protection Matrix above (`models:read`, `routing:read`, `chat`, `usage:read`). Principals lacking the required scope receive HTTP 403 Forbidden with canonical code `FORBIDDEN`.
>
> **Google OIDC Claims & Multi-Audience Invariants:** Google ID tokens must contain valid `iss`, `aud`, and `exp` claims. Per OpenID Connect Core 1.0 §3.1.3.7, if an ID token contains multiple audiences in an array, an `azp` (Authorized Party) claim MUST be present. If `azp` is present, it MUST match one of the configured Google client IDs. Insecure, missing, or mismatched `azp` claims are rejected with HTTP 401 Unauthorized.

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
  "conversation_id": "conv_drive_123",
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
  - `conversation_id`: Optional string. When provided:
    - Caller must be authenticated (`AuthPrincipal`) and must provide a valid, non-empty `X-Drive-Token` header. Missing principal or missing `X-Drive-Token` fails closed with HTTP 401 `Unauthorized` (`UNAUTHORIZED`).
    - The backend hydrates canonical model-agnostic chat history from Google Drive's hidden `appDataFolder`, verifying tenant ownership (`owner_subject:resource_id`).
    - Client-submitted messages are deduplicated against pre-existing canonical turns via deterministic prefix-matching.
    - Upon completion of buffered inference, the user turn and assistant response turn are atomically appended to the conversation, the revision is incremented monotonically, and the encrypted payload is persisted back to Google Drive before returning HTTP 200 OK.
    - In streaming inference (`POST /api/v1/chat/stream`), token deltas are accumulated into the complete assistant turn and persisted to Google Drive upon stream termination (`is_done` and stream EOF) without blocking the client SSE `event: done`.
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

## 7. Google Drive-Only Storage & Synchronization Endpoints

### 7.1 Architecture & Security Rules
- **Sole Permanent Store:** Google Drive (`appDataFolder`, `drive.appdata` scope) is the **sole permanent source of truth** for all user data (conversations, messages, attachments, library files, tombstones, and manifests).
- **Stateless Backend:** The backend maintains **zero permanent user database tables** (SQLite user tables `local_sync_conversations`, `local_sync_library`, `local_sync_metadata` have been removed). The local store is strictly an ephemeral in-memory working cache (`is_persistent() -> false`).
- **Save-Confirmation Protocol:** All create/update endpoints return HTTP 200/201 confirmation **only after** the payload has been encrypted with AES-256-GCM, uploaded to Google Drive, and indexed in the authoritative remote `manifest.json`.
- **Fail-Closed Authentication:** All user data operations strictly mandate valid Google authentication (`AuthPrincipal`) and the `X-Drive-Token` header.
- **Independent Library Lifecycle:** Deleting a conversation NEVER deletes its associated Library items. Library items can only be removed via explicit library deletion.

### 7.2 Configuration & Status Endpoints

#### GET /api/v1/drive/status
- **Authentication:** Optional for basic status; authenticated for user-scoped sync stats.
- **Response Schema (200 OK):**
```json
{
  "sync_enabled": true,
  "last_sync_time": "2026-10-22T14:30:00Z",
  "pending_conversations": 0,
  "pending_library_items": 0,
  "last_error": null
}
```

#### POST /api/v1/drive/config
- **Authentication:** Optional
- **Request Body Schema:**
```json
{
  "enabled": true
}
```
- **Response Schema (200 OK):** Returns updated `SyncStatus`. If `enabled: true` is requested when `DRIVE_ENCRYPTION_KEY` is not configured, returns `400 Bad Request`.

### 7.3 Conversation Endpoints

#### GET /api/v1/drive/conversations
- **Authentication:** Required (`Authorization: Bearer <google_id_token>`)
- **Headers:** `X-Drive-Token: <oauth_access_token>` (optional; if working cache is empty, hydrates from Drive)
- **Response Schema (200 OK):** Array of active `SyncConversation` objects.

#### GET /api/v1/drive/conversations/{id}
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>`
- **Response Schema (200 OK):** Hydrated `SyncConversation` object.

#### POST /api/v1/drive/conversations
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Request Body Schema:** `SyncConversation` JSON.
- **Response Schema (201 Created):** Returns confirmed `SyncConversation` persisted to Google Drive.

#### DELETE /api/v1/drive/conversations/{id}
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Response Schema (200 OK):** Confirms soft-delete tombstone written to Drive. Associated Library items remain intact.

### 7.4 Library Endpoints

#### GET /api/v1/drive/library
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (optional; hydrates from Drive on cache miss)
- **Response Schema (200 OK):** Array of active `LibraryItem` metadata objects.

#### GET /api/v1/drive/library/{id}
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>`
- **Response Schema (200 OK):** Hydrated `LibraryItem` metadata.

#### GET /api/v1/drive/library/{id}/content
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Response (200 OK):** Decrypted raw binary bytes with `Content-Type`, `Content-Disposition`, and SHA-256 checksum verification.

#### POST /api/v1/drive/library
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Request Body Schema:**
```json
{
  "filename": "document.pdf",
  "mime_type": "application/pdf",
  "content_base64": "JVBERi0xLjQK...",
  "conversation_id": "conv_123"
}
```
- **Response Schema (201 Created):** Confirms binary upload and metadata indexing to Drive.

#### DELETE /api/v1/drive/library/{id}
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Response Schema (200 OK):** Confirms explicit deletion of library item and remote Drive blob.

### 7.5 Synchronization, Restore & Session Endpoints

#### POST /api/v1/drive/sync
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Response Schema (200 OK):**
```json
{
  "uploaded_conversations": 1,
  "downloaded_conversations": 0,
  "uploaded_library_items": 1,
  "downloaded_library_items": 0,
  "conflicts_resolved": 0,
  "last_sync_time": "2026-10-22T14:35:00Z"
}
```

#### POST /api/v1/drive/restore
- **Authentication:** Required
- **Headers:** `X-Drive-Token: <oauth_access_token>` (mandatory)
- **Response Schema (200 OK):** Full bootstrap restoration from remote Drive manifest.

#### POST /api/v1/drive/logout
- **Authentication:** Required
- **Response Schema (200 OK):** Clears ephemeral in-memory working cache. Remote Drive files are never touched.

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

### 8.3 Personal Access Token & Dynamic Credential Lifecycle (Phase 12)
- **Scope Requirement:** `auth:tokens`

#### 8.3.1 Generate Token
- **Method & Path:** `POST /api/v1/auth/tokens`
- **Request Schema:**
```json
{
  "name": "Developer Laptop Key",
  "owner_subject": "optional_target_subject_for_admin_only",
  "scopes": ["chat:write", "usage:read"],
  "expires_in_secs": 2592000
}
```
- **Response Schema (201 Created):**
Plaintext token is returned exactly ONCE upon creation. Secrets are never persisted in plaintext.
```json
{
  "token": "pat_00000000000000000000000000000000_example",
  "metadata": {
    "id": "cred_019482710182",
    "name": "Developer Laptop Key",
    "owner_subject": "user_12345",
    "masked_token": "pat_1a2b...5c6d",
    "roles": ["user"],
    "scopes": ["chat:write", "usage:read"],
    "created_at": 1700000000,
    "expires_at": 1702592000,
    "revoked": false,
    "revoked_at": null
  }
}
```

#### 8.3.2 List Tokens
- **Method & Path:** `GET /api/v1/auth/tokens`
- **Isolation Rule:** Standard callers view strictly their own credentials. Querying another user returns 403 Forbidden. Plaintext tokens are NEVER exposed.
- **Response Schema (200 OK):**
```json
[
  {
    "id": "cred_019482710182",
    "name": "Developer Laptop Key",
    "owner_subject": "user_12345",
    "masked_token": "pat_1a2b...5c6d",
    "roles": ["user"],
    "scopes": ["chat:write", "usage:read"],
    "created_at": 1700000000,
    "expires_at": 1702592000,
    "revoked": false,
    "revoked_at": null
  }
]
```

#### 8.3.3 Inspect Token Metadata
- **Method & Path:** `GET /api/v1/auth/tokens/{id}`
- **IDOR Defense:** Requesting a credential owned by another subject returns `404 Not Found` to prevent existence enumeration.

#### 8.3.4 Rotate Token
- **Method & Path:** `POST /api/v1/auth/tokens/{id}/rotate`
- **Semantics:** Atomically invalidates the old token (immediately returns 401 on old token usage) and returns the new plaintext token once.

#### 8.3.5 Revoke Token
- **Method & Path:** `DELETE /api/v1/auth/tokens/{id}`
- **Response (204 No Content):** Atomically revokes token in persistent storage and validation cache.

---

### 8.4 Resource Ownership & Tenant Telemetry Isolation
- Standard users are strictly bound to their authenticated `AuthPrincipal.subject`.
- Usage endpoints (`/api/v1/usage/summary`, `/api/v1/usage/providers`, `/api/v1/usage/models`) automatically filter results by the caller's identity.
- Attempting to pass `?user_id=` or `?subject=` targeting another user's identifier returns `403 Forbidden` (`code: FORBIDDEN`, IDOR violation).
- Administrative principals (`AuthRole::Admin`) and service callers (`AuthRole::Service`) may view global aggregated telemetry or pass `?user_id=` filters.

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

---

## 10. Google Drive Synchronization & Library Endpoints (Phase 15 Implementation)

All endpoints in this section require authentication with scope `drive`. Additionally, operations that communicate with remote Google Drive require the `X-Drive-Token` header containing a valid Google OAuth access token with scope `https://www.googleapis.com/auth/drive.appdata`.

### 10.1 Drive Status & Configuration

#### 10.1.1 Get Drive Status
- **Method & Path:** `GET /api/v1/drive/status`
- **Headers:** `Authorization: Bearer <token>`
- **Response (200 OK):**
```json
{
  "sync_enabled": false,
  "last_synced_at": null,
  "pending_conversations": 1,
  "pending_library_items": 0,
  "active_conversations": 1,
  "active_library_items": 0,
  "is_syncing": false,
  "last_error": null
}
```

#### 10.1.2 Update Drive Configuration
- **Method & Path:** `POST /api/v1/drive/config`
- **Request Body:**
```json
{
  "sync_enabled": true
}
```
- **Response (200 OK):** Same schema as `GET /api/v1/drive/status`.

### 10.2 Bidirectional Synchronization & Restore

#### 10.2.1 Trigger Synchronization
- **Method & Path:** `POST /api/v1/drive/sync`
- **Headers:** `Authorization: Bearer <token>`, `X-Drive-Token: <oauth_access_token>`
- **Behavior:**
  - If `sync_enabled` is `false`, returns HTTP 400 Bad Request (`DRIVE_SYNC_DISABLED`).
  - Fetches or creates remote `manifest.json` in `appDataFolder`.
  - Performs bidirectional sync with Last-Write-Wins (LWW) conflict resolution.
  - Encrypts updated local conversations and library items with AES-256-GCM (AAD bound to `owner:resource_id`) and uploads to Google Drive.
  - Marks local items as clean (`is_dirty: false`).
- **Response (200 OK):**
```json
{
  "synced_at": "2026-10-22T12:00:00Z",
  "uploaded_conversations": 1,
  "downloaded_conversations": 0,
  "uploaded_library_items": 0,
  "downloaded_library_items": 0,
  "conflicts_resolved": 0
}
```

#### 10.2.2 Restore / Bootstrap Account
- **Method & Path:** `POST /api/v1/drive/restore`
- **Headers:** `Authorization: Bearer <token>`, `X-Drive-Token: <oauth_access_token>`
- **Behavior:** Restores all conversations and library index entries from Google Drive `appDataFolder` manifest.
- **Response (200 OK):**
```json
{
  "restored_at": "2026-10-22T12:00:00Z",
  "restored_conversations": 2,
  "restored_library_items": 1
}
```

### 10.3 Conversations Management

#### 10.3.1 List Conversations
- **Method & Path:** `GET /api/v1/drive/conversations`
- **Response (200 OK):** Array of conversation index entries:
```json
[
  {
    "id": "conv_01",
    "title": "Rust Architecture Discussion",
    "created_at": "2026-10-22T10:00:00Z",
    "updated_at": "2026-10-22T11:00:00Z",
    "message_count": 4,
    "deleted": false
  }
]
```

#### 10.3.2 Create or Upsert Conversation
- **Method & Path:** `POST /api/v1/drive/conversations`
- **Request Body:**
```json
{
  "id": "conv_01",
  "title": "Rust Architecture Discussion",
  "created_at": "2026-10-22T10:00:00Z",
  "updated_at": "2026-10-22T11:00:00Z",
  "messages": [
    {
      "id": "msg_01",
      "role": "user",
      "content": "Explain AES-256-GCM AAD binding.",
      "timestamp": "2026-10-22T10:00:00Z",
      "model": null,
      "tokens": null,
      "library_item_ids": ["lib_item_01"]
    }
  ],
  "associated_library_item_ids": ["lib_item_01"]
}
```
- **Response (200 OK):** Stored `SyncConversation` object.

#### 10.3.3 Get Conversation
- **Method & Path:** `GET /api/v1/drive/conversations/{id}`
- **Response (200 OK):** Full `SyncConversation` object. Returns 404 if deleted or not found.

#### 10.3.4 Delete Conversation
- **Method & Path:** `DELETE /api/v1/drive/conversations/{id}`
- **Critical Invariant:** Soft-deleting a conversation marks `deleted: true` on the conversation and sets dirty flag for sync. Associated Library items are **STRICTLY PRESERVED** and never deleted.
- **Response (204 No Content)**

### 10.4 Library Items Management

#### 10.4.1 List Library Items
- **Method & Path:** `GET /api/v1/drive/library`
- **Response (200 OK):** Array of `LibraryItem` metadata entries.

#### 10.4.2 Upload / Create Library Item
- **Method & Path:** `POST /api/v1/drive/library`
- **Request Body:**
```json
{
  "filename": "benchmark_data.json",
  "mime_type": "application/json",
  "content_base64": "eyJyZXN1bHQiOiA0Mn0=",
  "associated_conversation_id": "conv_01"
}
```
- **Path Traversal Defense:** `filename` is sanitized via `sanitize_filename`, stripping directory traversal (`../`, `..\`) and paths.
- **Integrity:** Automatically computes SHA-256 checksum and exact payload size.
- **Response (200 OK):** Created `LibraryItem` metadata.

#### 10.4.3 Download Library Item Content
- **Method & Path:** `GET /api/v1/drive/library/{id}/content`
- **Response (200 OK):** Raw binary octet-stream with `Content-Type: <mime_type>` and `Content-Disposition: attachment; filename="<sanitized_filename>"`.

#### 10.4.4 Explicitly Delete Library Item
- **Method & Path:** `DELETE /api/v1/drive/library/{id}`
- **Semantics:** Library items are deleted **strictly** through this explicit deletion endpoint.
- **Response (204 No Content)**

### 10.5 Session Teardown & Logout
- **Method & Path:** `POST /api/v1/drive/logout`
- **Semantics:** Clears local in-memory session and dirty cache for the authenticated owner. Remote Google Drive data is strictly preserved and never deleted.
- **Response (200 OK):** `{"status": "logged_out"}`

