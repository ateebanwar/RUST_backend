# AGENTS.md — Mandatory AI Agent Operating Guidelines

**Project:** `ai-backend` (High-Performance, Security-Focused Rust AI Backend)  
**Authority Level:** Mandatory Operational Contract  
**Governing Specification:** [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md)

---

## 1. Project Mission & Scope

Build a reusable, high-performance, security-focused Rust AI backend platform that serves multiple independent client applications through a stable, unified API:
1. **Flutter Mobile Application (Android):** Reference consumer for mobile chat, multimodal creation, and personal productivity.
2. **Custom AI-Powered IDE:** High-throughput developer tool for multi-file workspace reasoning, automated debugging, and inline code assistance.
3. **Editor Integrations (e.g., VS Code Extensions):** Contextual coding, terminal diagnostics, and chat assistance.
4. **Authorized API Consumers:** External tools adhering to authenticated API contracts.

The backend is completely decoupled from any single frontend. Provider integrations, model routing, usage tracking, Drive sync, and security controls live entirely within this Rust backend.

---

## 2. Source-of-Truth Hierarchy

All agents working on this codebase must strictly observe this authority sequence:

1. [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md): The primary, authoritative source of all functional and non-functional requirements.
2. [`AGENTS.md`](file:///d:/Rust%20Backend/ai-backend/AGENTS.md): Binding behavioral, security, and workflow constraints for agents.
3. Domain Markdown Documents:
   - [`ARCHITECTURE.md`](file:///d:/Rust%20Backend/ai-backend/ARCHITECTURE.md) (System topology & component contracts)
   - [`THREAT_MODEL.md`](file:///d:/Rust%20Backend/ai-backend/THREAT_MODEL.md) (Security boundaries, assets, & mitigations)
   - [`API_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/API_SPEC.md) (REST contracts, JSON schemas, & SSE protocols)
   - [`PROVIDER_INTEGRATION.md`](file:///d:/Rust%20Backend/ai-backend/PROVIDER_INTEGRATION.md) (Adapter traits & onboarding checklists)
   - [`SECURITY.md`](file:///d:/Rust%20Backend/ai-backend/SECURITY.md) (Baseline security policies & credential controls)
   - [`TESTING.md`](file:///d:/Rust%20Backend/ai-backend/TESTING.md) (Testing strategy & verification gates)
   - [`DEPLOYMENT.md`](file:///d:/Rust%20Backend/ai-backend/DEPLOYMENT.md) (Runtime execution & platform constraints)
   - [`DECISIONS.md`](file:///d:/Rust%20Backend/ai-backend/DECISIONS.md) (Approved Architecture Decision Records)
4. [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md) & [`CHANGELOG.md`](file:///d:/Rust%20Backend/ai-backend/CHANGELOG.md): Real-time tracking and historical record of executed work.

**Conflict Resolution Rule:** If any document conflicts with [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md), stop and report the contradiction immediately. Never silently pick an arbitrary interpretation or invent unapproved requirements.

---

## 3. Mandatory Workflow & Phased Gates

The project follows a strict 20-phase roadmap defined in [`MASTER_SPEC.md`](file:///d:/Rust%20Backend/ai-backend/MASTER_SPEC.md#L695-L757).

1. **Explicit Phase Authorization:** Do not implement code or start work on a phase until the user explicitly authorizes it.
2. **Phase Prerequisites:** Before beginning an authorized phase, inspect the prerequisites, read the relevant documentation, and verify that previous dependencies passed acceptance criteria.
3. **Scope Discipline:** Implement only the changes explicitly required by the active phase. Do not implement future phases ahead of schedule.
4. **Synchronous Verification:** Run formatting, linting, compilation, and tests after modifications. Report actual terminal results.
5. **Continuous Documentation Updates:** Update [`PROGRESS.md`](file:///d:/Rust%20Backend/ai-backend/PROGRESS.md), [`CHANGELOG.md`](file:///d:/Rust%20Backend/ai-backend/CHANGELOG.md), and [`DECISIONS.md`](file:///d:/Rust%20Backend/ai-backend/DECISIONS.md) upon phase milestone completion.

---

## 4. Security Rules (Non-Negotiable)

- **Zero Client Secrets:** Provider API keys and backend secrets must NEVER be transmitted to client applications, compiled into mobile/IDE binaries, or checked into version control.
- **Redaction by Default:** Redact all credentials, auth tokens, and sensitive headers from structured logs, diagnostics, and error responses.
- **Least-Privilege Drive Scopes:** Request strictly `drive.appdata` or `drive.file` for Google Drive synchronization. Never request broad Drive access.
- **Strict SSRF Defenses:** Disallow arbitrary user-specified outbound HTTP requests. Strictly allowlist provider hostnames and block RFC 1918 / loopback / link-local / cloud-metadata IPs.
- **Bounded Resource Limits:** Every external HTTP request must have an enforced timeout. Every inbound request must have a strict body size limit (e.g., 4.5 MB ceiling).
- **No Unsafe Claims:** Never claim a system is "100% secure" or "unhackable". Acknowledge residual risks and provide verification mechanisms.
- **No Security Downgrades:** Never bypass or weaken authentication, token verification, or authorization controls to make automated tests pass.

---

## 5. Rust Engineering Conventions

- **Toolchain:** Rust stable (specified in [`rust-toolchain.toml`](file:///d:/Rust%20Backend/ai-backend/rust-toolchain.toml)).
- **Async Execution:** Asynchronous I/O using Tokio and Axum. Keep CPU-bound tasks isolated from the async runtime.
- **Dependency Minimization:** Add dependencies only when strictly justified. Audit dependency trees for maintenance status, license compatibility, and vulnerabilities.
- **Decoupled Architecture:** Keep provider adapters, model routing, API presentation, quota tracking, and Drive synchronization in modular subcrates or isolated modules.
- **Typed Error Handling:** Use strongly typed error enums (e.g., using `thiserror` for library domains) and map them to standard API error envelopes.
- **Zero Paid Infrastructure:** Do not introduce paid managed databases, Redis clusters, or paid third-party services without explicit user approval.

---

## 6. Testing & Quality Gates

Every code change must satisfy standard verification checks before being reported as complete:
- Formatting: `cargo fmt --check`
- Compilation & Type Check: `cargo check`
- Unit & Integration Tests: `cargo test`
- Linter: `cargo clippy --all-targets --all-features -- -D warnings`

**Truth in Reporting Rule:** Never claim a test, check, or build passed unless the exact command was executed in the workspace and exited with status code `0`. Report failures, skipped tests, and unresolved issues transparently.

---

## 7. Strict Prohibitions

Unless explicitly requested and authorized by the user:
- DO NOT create or push to a remote GitHub repository.
- DO NOT deploy or provision cloud resources.
- DO NOT purchase or enable paid services.
- DO NOT delete user data, remote Drive files, or project history.
- DO NOT replace substantive documentation with superficial summaries.
- DO NOT make assumptions about platform capabilities without citing verifiable technical constraints.

---

## 8. Phase Completion Deliverables

For every authorized phase, the agent must present an evidence-based report containing:
1. Summary of changes made and rationales.
2. Complete list of files created, modified, or deleted.
3. Exact terminal commands executed and verbatim output.
4. Acceptance criteria verification table.
5. Known limitations, unresolved items, and pending ADRs.
6. Formal request for user approval to proceed to the next phase.
