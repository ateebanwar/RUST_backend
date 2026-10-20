# MASTER PROJECT SPECIFICATION
## AI Model Aggregation, Intelligent Routing & Secure Backend Platform

**Project Type:** Reusable Multi-Client AI Backend Platform  
**Backend Language:** Rust  
**Client Ecosystem:** Initial Reference Client: Flutter (Android); Planned Clients: Custom AI-powered IDE, VS Code / Editor Integrations, and Authorized API Consumers  
**Deployment Target:** Existing Vercel account (with Serverless / Container compatibility evaluation)  
**Chat Storage:** User-controlled Google Drive  
**Database Strategy:** No dedicated database in the initial architecture (Zero-Cost Infrastructure Constraint)  
**Cost Objective:** Zero recurring infrastructure cost wherever technically possible  
**Architecture Priority:** Security → Performance → Reliability → Extensibility → Answer Quality

**Document Status:** Master Specification (Synchronized & Multi-Client Aligned)  
**Roadmap:** 20 implementation phases  
**Development Strategy:** Backend-first, UI later

---

# 1. PROJECT VISION

Build a lightweight, high-performance, security-focused AI backend that aggregates multiple independent AI API repositories and providers into one unified, client-agnostic platform.

The backend serves as a centralized, secure API gateway and intelligent model router for multiple independent client applications:
1. **Flutter Mobile Application (Android):** Delivering a responsive, ChatGPT-like conversational experience, multimedia creation, and mobile productivity.
2. **Custom AI-Powered IDE:** Providing deep codebase reasoning, multi-file editing, automated debugging, and inline code generation.
3. **Editor Integrations (e.g., VS Code Extensions):** Providing contextual code completion, chat assistance, and technical problem-solving within developer workflows.
4. **Authorized API Consumers:** Any client conforming to the documented API contract and authentication protocol.

Users across all client interfaces must be able to ask questions, generate and edit images, solve complex coding problems, perform deep analysis, create documents and presentations, and use other supported AI capabilities from one consistent interface.

The system must intelligently select the most suitable model for each request, minimize unnecessary token consumption, display available models and their capabilities, track provider usage where supported, and automatically switch to compatible alternatives when a provider becomes unavailable or reaches a quota limit.

The backend must be designed as a modular platform. Adding another independent provider or repository in the future should require a small, well-defined integration adapter rather than redesigning the core routing or client layers.

The objective is to build a production-oriented backend, not a temporary demonstration or a basic API proxy.

# 2. PRIMARY OBJECTIVES

The following objectives govern all implementation decisions.

1. **Maximum practical performance:** Minimize request-processing overhead, memory consumption, CPU usage and unnecessary network operations.
2. **Strong security:** Apply security controls throughout the architecture, implementation, testing and deployment lifecycle.
3. **Intelligent model selection:** Automatically choose the most appropriate model for the user's request.
4. **Token efficiency:** Use expensive or high-capability models only when the task justifies them.
5. **Extensible integrations:** Support future repositories, providers, models and capabilities through modular adapters.
6. **Reliable execution:** Handle timeouts, rate limits, quota exhaustion and provider failures intelligently.
7. **Transparent model information:** Show users which model is being used and why it was selected.
8. **Google Drive-based history:** Keep user chat history in their own Drive so it survives application removal and device changes.
9. **Zero-cost preference:** Prefer genuinely free services, free API tiers and existing infrastructure. Do not introduce paid services without explicit approval.
10. **Backend-first development:** Complete and validate the backend foundation before building the Flutter interface.

# 3. PERFORMANCE REQUIREMENTS

Performance is a first-class architectural requirement.

## 3.1 API Response Performance

The backend should process requests with minimal overhead.

Targets to validate under a documented test environment:

- Internal routing and request-processing overhead: target below 10 ms at p95 when no external network operation is required.
- Lightweight metadata endpoints: target below 50 ms at p95 under normal test conditions.
- Cached model-registry responses: target below 50 ms at p95 under normal test conditions.
- Authentication, validation and routing must avoid unnecessary work.
- Network requests to providers must use appropriate connection pooling, timeouts and concurrency limits.
- Avoid repeated model discovery, unnecessary serialization and redundant provider calls.

These are engineering targets, not guaranteed results. Actual latency must be measured on the deployed environment.

## 3.2 AI Response Performance

Total response time depends on the selected provider, model, queue, network and response length.

The system must:

- Select fast models for simple tasks.
- Avoid unnecessary reasoning calls.
- Stream generated text when supported by the provider.
- Avoid waiting for unrelated providers when a suitable model is already available.
- Apply bounded retries and strict deadlines.
- Avoid retrying requests that are already completed.
- Support cancellation when the client disconnects, wherever possible.

The backend must distinguish its own processing time from external model inference time.

## 3.3 Resource Efficiency

- Use Rust and asynchronous I/O.
- Prefer a lightweight HTTP framework such as Axum with Tokio.
- Minimize dependencies and avoid unnecessary background services.
- Avoid memory-heavy in-memory caches and unbounded queues.
- Use bounded concurrency and request-body limits.
- Avoid requiring a permanently running server when the selected hosting platform does not support it.
- Measure cold-start latency, memory consumption, CPU usage and throughput.

## 3.4 Performance Testing

Test and report:

- p50, p95 and p99 latency.
- Requests per second.
- Concurrent requests.
- Error and timeout rates.
- Memory consumption.
- CPU usage.
- Cold-start performance.
- Provider-specific latency.
- Performance under rate limiting and failure conditions.

Never claim a performance target has been achieved without test results.

# 4. PROVIDER AND REPOSITORY INTEGRATION

## 4.1 Core Requirement

The backend must support multiple independent GitHub repositories and AI API providers through a common integration architecture.

Each repository or provider may expose different endpoints, authentication mechanisms, model formats, response structures and quota information.

These differences must be handled by provider-specific adapters.

## 4.2 Adapter Architecture

Each adapter should implement a standardized internal contract covering, where supported:

- Provider identification.
- Authentication configuration.
- Model discovery.
- Capability discovery.
- Request translation.
- Response normalization.
- Streaming.
- Error normalization.
- Rate-limit information.
- Quota and usage information.
- Timeout handling.
- Health and availability status.
- Provider-specific feature support.

One provider's failure must not crash or disable the entire backend.

## 4.3 Adding Future Repositories

The architecture must support adding a new provider without modifying unrelated routing, security or chat modules.

A new integration should require only the necessary adapter implementation, configuration and tests.

Where possible, provider metadata should be configuration-driven. Provider-specific logic must remain isolated.

Untrusted repositories must never be executed automatically merely because their URLs have been added. Integrations must be reviewed, pinned to known versions or commits where appropriate, tested and explicitly enabled.

## 4.4 Repository Independence

Maintain a provider registry that distinguishes:

- Original projects.
- Forks.
- Mirrors.
- Different deployments of the same upstream service.
- Providers that share upstream infrastructure or API quotas.

The system must not assume that two repositories offer independent capacity simply because their repository URLs differ.

# 5. CENTRAL MODEL REGISTRY

The backend must maintain a unified registry of discovered and configured models.

Each model entry should contain the following fields whenever the information is available:

- Model ID.
- Display name.
- Provider ID.
- Repository or integration source.
- Supported capabilities.
- Input modalities.
- Output modalities.
- Context-window information.
- Maximum output size.
- Streaming support.
- Tool-calling support.
- Structured-output support.
- Reasoning capability.
- Coding suitability.
- Image-generation capability.
- Image-editing capability.
- Video-generation capability.
- Audio capabilities.
- Document-generation suitability.
- Estimated speed category.
- Cost category.
- Provider availability.
- Rate-limit status.
- Quota information.
- Last successful health check.
- Metadata source and last update time.

Unknown properties must be represented as unknown, not fabricated.

## 5.1 Model Selection & UI Metadata Requirements

The backend must expose normalized model metadata enabling any client application (the future Flutter mobile application, the custom IDE, and editor extensions) to render an intuitive, searchable model selector.

Each model listing must provide standardized fields so frontends can display:

**Model Name — Provider — Capability — Speed/Cost Category — Availability**

Standardized capability and speed categories include:

- Fast / Lightweight.
- Balanced.
- Advanced Coding.
- Deep Reasoning.
- Image Generation.
- Image Editing.
- Video Generation.
- Document and Presentation Tasks.

These categories must be based on actual capabilities and measured or documented characteristics, not invented labels.

The metadata must also allow client selectors to distinguish unavailable models, models requiring configuration, and models that cannot handle the active modality or task.

# 6. INTELLIGENT MODEL ROUTING

## 6.1 Automatic Routing

Automatic routing is the default operating mode.

For each user request, the router should:

1. Identify the task type.
2. Estimate complexity.
3. Identify required input and output modalities.
4. Determine capability and quality requirements.
5. Find compatible available models.
6. Evaluate latency, cost category, quota status and reliability.
7. Select the best suitable model.
8. Execute the request.
9. Record usage and outcome where measurable.
10. Return the result and routing metadata.

## 6.2 Task Categories

The router must support the following categories as compatible models become available:

- Simple questions and general conversation.
- Complex reasoning and analysis.
- Coding and debugging.
- Architecture and software engineering.
- Mathematics and technical problem-solving.
- Web search and research.
- Image generation.
- Image editing.
- Video generation.
- Audio processing and generation.
- Document creation.
- PowerPoint generation.
- Data extraction and structured output.
- Other future task categories.

## 6.3 Complexity-Based Selection

Simple prompts should use fast, lightweight models when they can provide an adequate answer.

Complex coding, multi-step reasoning and difficult analysis should use stronger models when justified.

Image, video, audio and document tasks must use compatible models or tools rather than text-only models that cannot perform the requested operation.

The system should not select the most expensive or resource-intensive model by default.

## 6.4 Manual Model Selection

The system must support both:

- Automatic model selection.
- Explicit user-selected model selection.

The API must accept a routing mode and, when applicable, a requested model ID.

The backend must validate that the selected model exists, is enabled and supports the requested task.

The selected model and the actual executing model must be reported separately if failover changes the execution model.

## 6.5 Explainable Routing

The backend should return concise routing metadata, such as:

- Requested model or automatic mode.
- Selected model.
- Provider.
- Task category.
- Selection reason.
- Fallback occurrence.
- Actual execution status.

Sensitive internal security details and provider credentials must never be exposed.

# 7. TOKEN OPTIMIZATION

Token efficiency is a major requirement.

The system must:

- Prefer appropriately sized models for simple requests.
- Avoid unnecessary repeated model calls.
- Avoid sending irrelevant conversation history.
- Estimate token usage before execution when supported.
- Limit context to the information needed for the task.
- Track input and output tokens separately when reported.
- Support configurable response-length limits.
- Avoid unnecessary retries that generate duplicate charges or consume quotas.
- Reuse safe, reusable metadata and model-registry results.
- Avoid caching private user prompts by default.

When a task genuinely requires advanced reasoning, the router should prioritize answer quality over blindly choosing the smallest model.

Token optimization must not silently degrade important user requirements.

# 8. QUOTA TRACKING AND PROVIDER USAGE

The system must track usage separately for each provider, model and configured credential identity where the provider supports it.

Record available information such as:

- Input tokens.
- Output tokens.
- Total tokens.
- Request count.
- Rate-limit events.
- Quota-exhaustion events.
- Remaining quota, if explicitly reported.
- Reset time, if explicitly reported.
- Last successful request.
- Provider errors.
- Fallback events.

The UI must distinguish:

- Reported provider quota.
- Estimated usage.
- Locally counted requests.
- Unknown remaining quota.
- Known reset time.
- Estimated reset time.

Never present an estimate as an official provider value.

Where a provider exposes no remaining-quota endpoint, the system should show the most accurate available usage information without inventing a remaining-token count.

# 9. AUTOMATIC FAILOVER AND RELIABILITY

The backend must switch to another compatible provider or model when the selected integration fails and a valid alternative exists.

Supported failure categories should include:

- Rate limits.
- Quota exhaustion.
- Connection failures.
- Timeouts.
- Provider outages.
- Temporary server errors.
- Model unavailability.
- Unsupported capabilities.

Requirements:

- Bounded retries.
- Strict request deadlines.
- Exponential backoff with jitter where appropriate.
- Provider health tracking.
- Temporary circuit breaking for repeatedly failing providers.
- Compatibility checks before fallback.
- Loop prevention.
- Idempotency controls where applicable.
- Duplicate-execution protection where feasible.
- Clear error reporting when no compatible alternative remains.

The backend must never claim a request succeeded before a successful result is received.

Retries must account for whether the upstream operation is safe to repeat, particularly for image generation and other operations that may consume substantial quota.

# 10. RUST BACKEND ARCHITECTURE

Use a modular, asynchronous Rust architecture.

Suggested technology choices:

- Rust stable toolchain.
- Axum for HTTP routing.
- Tokio for asynchronous execution.
- Serde for serialization and deserialization.
- Tower middleware where appropriate.
- A well-maintained HTTP client with connection pooling.
- Structured tracing for diagnostics.
- Strict configuration validation at startup.

Exact dependencies must be evaluated for maintenance, security, resource usage and compatibility before adoption.

Suggested logical modules:

- `api`
- `config`
- `auth`
- `security`
- `providers`
- `adapters`
- `models`
- `registry`
- `router`
- `quota`
- `usage`
- `failover`
- `chat`
- `drive`
- `observability`
- `errors`
- `testing`

The implementation should use clear module boundaries, typed interfaces, centralized error handling and minimal coupling.

Avoid creating a monolith where adding one provider requires editing every subsystem.

# 11. SECURITY REQUIREMENTS

Security must be integrated from the first phase, not added at the end.

## 11.1 Threat Modeling

Document:

- Assets and secrets.
- Trust boundaries.
- Attack surfaces.
- Authentication flows.
- Authorization rules.
- External provider dependencies.
- Data flows.
- Abuse scenarios.
- Failure modes.
- Security controls and residual risks.

Maintain the threat model as the architecture changes.

## 11.2 Authentication and Authorization

- Use a properly validated authentication mechanism.
- Verify token signatures, issuer, audience and expiry as applicable.
- Do not trust a user ID supplied by the client without verifying ownership.
- Enforce authorization on every protected operation.
- Protect administrative functionality separately.
- Implement secure session handling.
- Avoid logging access tokens.

Google Sign-In must not automatically grant access to arbitrary user data. The requested Drive permissions must be explicitly authorized.

## 11.3 API Key Security

- Keep provider API keys out of the Flutter application.
- Keep secrets out of source control.
- Use the deployment platform's supported secret configuration.
- Never return secrets through API responses.
- Redact credentials from logs and error messages.
- Rotate credentials when necessary.
- Validate configuration before activating integrations.
- Never expose secret values in diagnostics.

## 11.4 Request Security

- Strict request-body size limits.
- Input validation and type checking.
- Timeouts on external operations.
- Rate limiting.
- Bounded concurrency.
- Request-count and abuse controls.
- Safe error messages.
- Protection against malformed and oversized requests.
- Protection against request smuggling and unsafe proxy behavior where relevant.
- Restrictive CORS configuration when browser clients are introduced.
- Secure HTTP transport.

## 11.5 SSRF and External URLs

Provider URLs must come from trusted configuration, not arbitrary user input.

Where dynamic URLs are necessary, validate schemes, hosts, ports and resolved addresses. Prevent access to private, loopback, link-local and cloud metadata addresses unless a specific, justified exception exists.

Revalidate redirects and DNS resolution as appropriate.

## 11.6 Dependency and Supply-Chain Security

- Minimize dependencies.
- Audit dependency trees.
- Track security advisories.
- Pin and review important dependency versions.
- Use lockfiles.
- Review external repositories before integration.
- Avoid automatically executing downloaded code.
- Scan source and build artifacts.
- Establish a process for updating vulnerable dependencies.

## 11.7 Testing and Security Validation

Include:

- Unit tests.
- Integration tests.
- Property-based testing where useful.
- Fuzz testing of parsers and request boundaries.
- Authentication and authorization tests.
- Abuse and rate-limit tests.
- Dependency audits.
- Static analysis.
- Secret scanning.
- Security regression tests.
- Penetration testing before production release.

No implementation can honestly guarantee that every possible vulnerability has been eliminated. The objective is measurable risk reduction, continuous validation and rapid remediation.

# 12. GOOGLE SIGN-IN AND GOOGLE DRIVE CHAT SYNC

## 12.1 Storage Objective

User chat history should be stored in the user's Google Drive rather than requiring a separate paid database.

The backend must provide APIs and contracts for securely creating, retrieving, updating, and synchronizing chat history.

Authorized client applications (including the Flutter Android application, custom IDE, and editor extensions) should be able to restore the user's conversational history after reinstalling the application, changing devices, or switching development environments.

### 12.1.1 Architectural Comparison: Client-Direct vs. Backend-Mediated Drive Access

Two primary patterns exist for Google Drive integration under a zero-cost serverless model:

1. **Backend-Mediated Synchronization:**
   - *Flow:* Client authorizes Google Drive → Sends OAuth access token to the Rust backend → Backend calls Google Drive REST API to store/retrieve chat JSON objects → Backend returns normalized responses to the client.
   - *Advantages:* Centralized conflict detection, uniform serialization across all client types (Flutter, IDE, extensions), centralized encryption-at-rest key management, and no client-side Drive API SDK dependencies.
   - *Trade-offs:* Backend handles OAuth token transmission and Drive I/O within serverless execution deadlines.
2. **Client-Direct Synchronization with Backend Data Schemas:**
   - *Flow:* Client obtains Drive OAuth token directly → Backend defines canonical schemas and conflict resolution algorithms → Client writes/reads directly to/from user's Google Drive (`appDataFolder`).
   - *Advantages:* Zero backend execution overhead, completely removes Drive OAuth tokens from the backend transit path, and immune to serverless execution timeout limits during large sync operations.
   - *Trade-offs:* Each frontend must implement Google Drive API client calls.

**Approved Recommendation:** Implement **Backend-Mediated Synchronization** as the primary standardized API contract (`/api/v1/drive/sync`), while designing the chat serialization format to be fully client-portable so that client-direct fallback remains technically viable if serverless execution time limits dictate.

## 12.2 Authentication and Permissions Across Client Types

The backend serves multiple client types and must support appropriate authentication mechanisms:

1. **Mobile & Web Clients (Flutter):**
   - User identity authenticated via Google Sign-In (OAuth 2.0 / OIDC ID tokens).
   - Backend validates the ID token signature, issuer (`https://accounts.google.com`), audience (client ID), and expiration.
   - User identity is verified on every request without trusting client-asserted user IDs.
2. **Desktop IDE & Editor Integrations (Custom IDE, VS Code Extension):**
   - Support Personal Access Tokens (PATs) / Backend API Keys for headless or developer CLI environments.
   - Support OAuth 2.0 Device Authorization Grant (RFC 8628) or local loopback authentication where a browser is available.
3. **Google Drive Scopes & Least Privilege:**
   - Request strictly the narrowest necessary Google Drive scope: prefer `https://www.googleapis.com/auth/drive.appdata` (private Application Data folder hidden from user Drive view, preventing accidental user tampering) or `https://www.googleapis.com/auth/drive.file` (access only to files created by the app).
   - Never request full `drive` or `drive.readonly` scope.
   - Clearly separate user authentication from Drive authorization: a user may authenticate to chat without granting Drive synchronization permissions.
   - Validate and safely handle token refresh, revoked scopes, and expired credentials without service crashes.

## 12.3 Synchronization Requirements

- Stable identifiers for chats and messages.
- Incremental synchronization where practical.
- Conflict detection and resolution.
- Retry-safe writes.
- Duplicate prevention.
- Pagination for large histories.
- Recovery after interrupted synchronization.
- File-version or revision handling where available.
- Secure handling of access and refresh tokens.
- Graceful behavior when Drive is temporarily unavailable.

The backend should not assume every device has the same local state.

## 12.4 Logout and Uninstall

Logout should clear the local session and sensitive cached data.

Logging out must not silently delete chat files from Drive.

Uninstalling the application should not delete history already saved to Drive. After reinstallation and renewed authorization, the application should be able to restore the history.

## 12.5 Data Privacy

- Store only the information necessary for the product.
- Do not log complete conversations by default.
- Do not send chat history to unrelated providers.
- Send only the context required for the requested task.
- Provide clear data deletion and account-disconnection behavior.
- Avoid creating an additional permanent server-side conversation database unless separately approved.

# 13. FILE GENERATION AND MULTIMODAL CAPABILITIES

The backend should support image, video, audio and document workflows through capability-aware adapters.

## 13.1 Images and Video

Support image generation, image editing and video generation when an integrated provider exposes those capabilities.

The API must distinguish text responses from binary files, media URLs and asynchronous generation jobs.

Handle supported file sizes, timeouts, task status, retrieval and errors.

## 13.2 Documents and Presentations

For Word documents, PDFs, PowerPoint presentations and other supported formats, use suitable file-generation libraries or tools in addition to AI models.

The backend should distinguish between generating document content and generating the actual downloadable file.

## 13.3 Extensibility

New modalities must be added through explicit capability interfaces rather than assumptions that every model can handle every task.

# 14. REST API DESIGN

The backend must expose versioned, documented endpoints.

Proposed endpoint families:

- `/api/v1/health`
- `/api/v1/ready`
- `/api/v1/models`
- `/api/v1/providers`
- `/api/v1/chat/completions`
- `/api/v1/chat/stream`
- `/api/v1/usage`
- `/api/v1/quotas`
- `/api/v1/routing/preview`
- `/api/v1/drive/chats`
- `/api/v1/drive/sync`

These are proposed contracts, not final implementations. Each endpoint must have a defined request schema, response schema, authentication requirement, rate limit and error contract.

Requirements:

- Versioned APIs.
- Consistent JSON structures.
- Stable error codes.
- Request validation.
- Pagination where necessary.
- Streaming where supported.
- OpenAPI documentation.
- Explicit timeout behavior.
- Safe handling of malformed requests.
- No secret values in responses.

# 15. DEPLOYMENT AND ZERO-COST STRATEGY

The initial deployment target is the user's existing Vercel account, with a strict cost objective of zero recurring infrastructure cost wherever technically feasible.

## 15.1 Technical Evaluation of Vercel for Rust Backends

Deploying Rust to Vercel requires understanding the underlying serverless execution model:

- **Runtime & Execution Architecture:**
  - Vercel executes serverless functions on AWS Lambda microVMs.
  - Rust binaries can be compiled for the `provided.al2023` or `provided.al2` Lambda runtime using community builders (such as `vercel-rust` / `cargo-lambda`).
  - Axum can be integrated seamlessly using the `lambda_http` crate bridge, which converts incoming AWS Lambda API Gateway / Vercel HTTP events into standard Tower/Hyper/Axum requests.
- **Platform Constraints & Limits:**
  - *Execution Timeout:* Hobby tier functions are hard-limited to 15 seconds (Pro tier up to 60s/300s). AI completions or streaming responses exceeding 15 seconds will be abruptly terminated by the edge proxy.
  - *Payload Limit:* 4.5 MB maximum request and response payload size.
  - *Cold Starts & Ephemeral State:* MicroVM instances spin down after inactivity. Memory is entirely ephemeral; instances do not share state.
  - *Streaming Support:* Vercel supports Server-Sent Events (SSE) via chunked HTTP transfer encoding, but the total connection duration remains bounded by the maximum function duration.
  - *Stateless Implications:* In-memory connection pools, persistent rate-limit counters, and dynamic circuit breakers lose state across cold starts and concurrent lambda invocations.

## 15.2 Hosting Architecture Comparison

| Dimension | Option 1: Vercel Serverless Function (Current Target) | Option 2: Long-Running Rust Container (e.g., Fly.io / Render / VPS) | Option 3: Split Architecture |
|---|---|---|---|
| **Hosting Cost** | $0.00 / month (leveraging existing Vercel account) | $0.00 (on limited free tiers) or $3–$7 / month | Mixed |
| **Timeout Limits** | 15s (Hobby) / 60s (Pro) | Indefinite / Configurable (minutes or hours) | Short-lived on Vercel, long on worker |
| **Streaming / SSE** | Supported up to execution timeout limit | Fully supported with keep-alive & cancellation | Dependent on route |
| **State Management** | Ephemeral only (stateless heuristics or client-state) | In-memory caches, connection pooling, live circuit breakers | Complex distributed state |
| **Operational Overhead** | Extremely low (git push to deploy, zero server management) | Moderate (Dockerfiles, health monitors, restarts) | High (two distinct deployment pipelines) |

## 15.3 Approved Architectural Recommendation

To honor the zero-cost objective while maintaining performance and reliability:

1. **Dual-Target Core Architecture:**
   - Implement the backend logic inside a decoupled `lib.rs` and modular subcrates, exposing an `axum::Router`.
   - Provide two build targets:
     - `src/main.rs`: Standard long-running Axum HTTP daemon (for local development, automated testing, and optional container deployment).
     - Serverless Entrypoint (e.g. `api/index.rs` or `cargo-lambda` bootstrap): Serverless adapter bridging incoming requests via `lambda_http` to the same `axum::Router`.
2. **Stateless Fallback & Heuristic State Design:**
   - Under Vercel Serverless, design rate-limiting, usage tracking, and circuit-breaking to operate gracefully with ephemeral state.
   - Circuit breakers use fast-decay heuristics per warm instance and rely on provider error responses rather than blocking cluster-wide coordination.
   - Chat history persistence is offloaded to user-controlled Google Drive, keeping the backend entirely stateless.
3. **Execution Deadline Protection:**
   - Impose strict upstream provider request timeouts (e.g., 10 seconds for standard completions) to guarantee responses finish well before Vercel's 15-second execution ceiling.
   - For long-running multimodal or deep reasoning tasks that cannot complete in 15 seconds, document containerized deployment as the approved operational upgrade.

Free tier access must never be assumed to be unlimited or permanent. All resource limits must be tracked and respected.

# 16. OBSERVABILITY AND DIAGNOSTICS

Provide structured, privacy-conscious operational visibility.

Track:

- Request IDs.
- Request duration.
- Routing duration.
- Provider latency.
- Provider failures.
- Rate-limit events.
- Quota exhaustion.
- Failover attempts.
- Model-selection decisions.
- Memory and resource measurements where available.
- Application startup and configuration errors.

Logs must redact credentials and sensitive user information.

The system should support health checks, useful diagnostic errors and actionable operational metrics without depending on expensive monitoring services.

# 17. TESTING AND ACCEPTANCE CRITERIA

Every implementation phase must have explicit acceptance criteria.

The backend is not considered complete merely because it compiles.

Required validation includes:

- Successful build.
- Passing automated tests.
- Strict request validation.
- Correct authentication and authorization.
- Provider adapter contract tests.
- Model registry consistency.
- Routing decision tests.
- Quota tracking tests.
- Failover and retry tests.
- Timeout and cancellation tests.
- Drive synchronization tests.
- Security regression tests.
- Load and latency tests.
- Deployment smoke tests.

Any claimed capability must have a working implementation or be clearly marked as pending.

# 18. TWENTY-PHASE IMPLEMENTATION ROADMAP

## Phase 1 — Requirements and Threat Model
Finalize requirements, trust boundaries, assets, threat scenarios, nonfunctional requirements and acceptance criteria.

## Phase 2 — Architecture and API Contracts
Define module boundaries, request and response schemas, provider contracts, error formats and API versioning.

## Phase 3 — Rust Core Foundation
Establish the Rust project, asynchronous runtime, configuration handling, structured errors, tracing and basic HTTP endpoints.

## Phase 4 — Provider Adapter Framework
Implement the standardized provider interface, integration lifecycle, timeout behavior and adapter contract tests.

## Phase 5 — Central Model Registry
Build model metadata, capability discovery, normalization, availability state and model-list APIs.

## Phase 6 — Task Classification
Implement request categorization and complexity estimation with validated routing inputs.

## Phase 7 — Intelligent Model Router
Select compatible models according to task requirements, capability, latency, reliability and cost category.

## Phase 8 — Quota and Usage Tracking
Normalize provider usage, rate limits, quota information and reset-time metadata where available.

## Phase 9 — Failover and Reliability
Implement bounded retries, provider health tracking, circuit breaking, fallback selection and duplicate-execution safeguards.

## Phase 10 — Token Optimization
Implement context management, token estimation, response limits and efficient model-selection policies.

## Phase 11 — Authentication
Implement authentication and identity verification for the backend and its protected endpoints.

## Phase 12 — Authorization and Secret Management
Implement resource ownership, access controls, credential handling and secret redaction.

## Phase 13 — Abuse Prevention
Implement rate limits, bounded concurrency, request limits, timeouts and abuse controls.

## Phase 14 — Security Hardening
Conduct security reviews, dependency audits, fuzzing, static analysis and security regression testing.

## Phase 15 — Google Drive Integration
Implement the required authorization, Drive access and safe token lifecycle.

## Phase 16 — Chat History and Synchronization
Implement chat data formats, stable identifiers, synchronization, conflict handling and restoration.

## Phase 17 — Observability and Diagnostics
Complete structured logging, metrics, health checks, request tracing and operational diagnostics.

## Phase 18 — Comprehensive Automated Testing
Complete unit, integration, provider-contract, synchronization, failure-mode and end-to-end backend tests.

## Phase 19 — Performance and Security Validation
Run load tests, latency analysis, resource profiling and penetration testing; resolve identified issues.

## Phase 20 — Production Deployment and Readiness
Deploy to the supported Vercel execution model, configure secrets, validate production behavior and document operations, recovery and maintenance.

Each phase must produce tangible artifacts, tests and a completion report. A phase must not be marked complete merely because its code has been written.

# 19. DEVELOPMENT RULES FOR AI CODING AGENTS

Any AI agent working on this project must follow these rules:

1. Read the master specification before implementing a phase.
2. Read the current progress and architecture documents.
3. Follow existing project decisions unless a documented change is approved.
4. Do not silently remove or weaken requirements.
5. Do not introduce paid services without explicit approval.
6. Do not place provider secrets in source code or mobile builds.
7. Do not claim that tests passed without running them.
8. Do not claim that security is guaranteed or that the application is unhackable.
9. Avoid unnecessary dependencies and architectural complexity.
10. Implement explicit error handling and input validation.
11. Keep modules independently testable.
12. Add tests for new behavior and security-sensitive changes.
13. Update documentation when contracts or architecture change.
14. Maintain a record of completed work, unresolved issues and known limitations.
15. Do not silently substitute mocked behavior for real functionality.
16. Do not claim a provider capability exists until it has been verified.
17. Preserve backwards compatibility for published API contracts where practical.
18. Stop and report a security-critical conflict rather than bypassing a security control.
19. Prefer measurable evidence over assumptions.
20. Keep all changes aligned with the approved phase and acceptance criteria.

# 20. REQUIRED PROJECT DOCUMENTS

The repository must contain these project-management documents:

- `MASTER_SPEC.md` — complete requirements and architectural objectives.
- `AGENTS.md` — instructions for AI coding agents.
- `PROGRESS.md` — current phase, completed work and next tasks.
- `ARCHITECTURE.md` — current architecture and module relationships.
- `THREAT_MODEL.md` — assets, attack surfaces, risks and mitigations.
- `API_SPEC.md` — REST API contracts.
- `PROVIDER_INTEGRATION.md` — instructions for adding and validating providers.
- `SECURITY.md` — security policies and vulnerability-handling process.
- `TESTING.md` — testing strategy and execution commands.
- `DEPLOYMENT.md` — deployment configuration and recovery procedures.
- `DECISIONS.md` — significant technical decisions and their rationale.
- `CHANGELOG.md` — notable changes across versions.

Do not duplicate the full specification into every document. Keep the master specification authoritative and reference it from the other files.

# 21. INITIAL PROJECT STRUCTURE

The initial repository should use a clean, modular structure similar to:

```text
ai-backend/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── MASTER_SPEC.md
├── AGENTS.md
├── PROGRESS.md
├── ARCHITECTURE.md
├── THREAT_MODEL.md
├── API_SPEC.md
├── PROVIDER_INTEGRATION.md
├── SECURITY.md
├── TESTING.md
├── DEPLOYMENT.md
├── DECISIONS.md
├── CHANGELOG.md
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── api/
│   ├── config/
│   ├── auth/
│   ├── security/
│   ├── providers/
│   ├── adapters/
│   ├── models/
│   ├── registry/
│   ├── router/
│   ├── quota/
│   ├── usage/
│   ├── failover/
│   ├── chat/
│   ├── drive/
│   ├── observability/
│   └── errors/
└── tests/
```

This structure is a starting proposal. Final module boundaries should follow the architecture established in Phases 1 and 2.

# 22. PROJECT COMPLETION DEFINITION

The backend is considered ready for multi-client frontend integration (including the Flutter Android application, custom IDE, and editor integrations) when:

- All required API contracts are implemented and documented.
- Multi-client authentication (Google Sign-In for mobile, PAT/API keys for IDE/extensions) is validated.
- Provider integrations can be added through the defined adapter framework.
- The model registry exposes verified model metadata across all supported modalities.
- Automatic and manual routing work as specified.
- Token and quota reporting accurately distinguish known, estimated and unavailable information.
- Failover operates within defined retry and deadline limits without duplicate executions.
- Authentication and authorization tests pass.
- Google Drive synchronization preserves user history across supported restoration scenarios.
- Security testing has been performed and critical findings have been resolved.
- Performance has been measured against the documented targets.
- Deployment has been validated in the actual target environment.
- The remaining known risks and unsupported capabilities are documented.
- The project has a repeatable build, test, deployment and recovery process.

# 23. FINAL ENGINEERING PRINCIPLE

Build the smallest architecture that satisfies the full specification without sacrificing security, reliability or future extensibility.

Do not optimize only for a fast demonstration. Build for measurable performance, controlled resource consumption, modular integration, verifiable security and maintainable evolution.

Every major design choice must have a clear reason. Every critical feature must have tests. Every provider capability must be verified. Every security claim must be supported by evidence.

**The first deliverable is the documented and tested Rust foundation. Client UI implementations (Flutter, custom IDE, and editor plugins) proceed only after the backend contracts and core behavior are stable.**
