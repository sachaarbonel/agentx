# AGENTS Guidance

Scope: This file applies to the entire repository.

## Overview
- This crate (`agentx`) integrates a hosted computer-use model via the OpenAI Responses API and drives a Chromium browser via `chromiumoxide`.
- Core modules:
  - `src/cua.rs`: API client, request/response shaping, action decoding.
  - `src/browser.rs`: Browser control helpers (navigation, input, screenshots).
  - `src/agent.rs`: Orchestrates the loop between the model and the browser.
- Example entrypoint: `examples/quickstart.rs`.

## Run & Develop
- Prereqs: stable Rust (edition 2021), Chrome/Chromium available on PATH.
- Env vars:
  - `OPENAI_API_KEY` (required)
  - `OPENAI_BASE_URL` (optional, default `https://api.openai.com/v1`)
  - `OPENAI_CUA_MODEL` (optional, default `computer-use-preview`)
- Build: `cargo build`
- Example: `RUST_LOG=info cargo run --example quickstart`

## Code Style & Conventions
- Keep changes minimal and focused on the task; avoid broad refactors unless requested.
- Maintain the public API exported by `lib.rs` (`Agent`, `AgentConfig`, `Browser`, `BrowserConfig`, `CuaClient`, `CuaConfig`). Do not introduce breaking changes without explicit instruction.
- Error handling: prefer `Result<T, anyhow::Error>` at integration/edge layers; consider `thiserror` for reusable library error types. Avoid `unwrap()`/`expect()` in library code.
- Logging: use `tracing` (`info!`, `warn!`) instead of `println!`. Examples can initialize `tracing-subscriber`.
- Formatting: follow `rustfmt` defaults. If adding clippy lint fixes, keep them minimal and scoped.
- Platform assumptions: avoid OS-specific behavior; gate where necessary with `cfg!(target_os = ...)`.
- No inline copyright or license headers.

## Dependencies
- Be conservative adding new crates. Favor std and existing dependencies.
- If a new dependency is required:
  - Justify its use (size, maintenance, features) in the PR description.
  - Disable default features where possible and enable only what’s needed.
  - Use semver-stable releases; avoid git dependencies.

## Browser Integration
- `Browser::launch` isolates a unique user-data-dir to avoid Chromium profile locks. Do not remove this.
- Default runs headless; examples may set `headless: false` for interactive demos.
- Prefer CDP-driven inputs (`input.*`, `InsertText`, `DispatchMouseEvent`) over JS injection; JS is acceptable where CDP lacks an equivalent.
- Keep `wait_for_stable` simple and deterministic; avoid long sleeps. If you must add waits, prefer event-driven checks.

## OpenAI Responses API
- Requests are built in `CuaClient::turn` and `send_computer_output`.
- Tools: some deployments expect `display_width/display_height` instead of `display_width_px/display_height_px`; `normalize_tools` handles this—keep it.
- Do not log sensitive fields (API keys, raw auth headers). Avoid embedding secrets in code.

## Extending Actions (CUA)
When adding a new tool action end-to-end:
1. Extend `CuaAction` in `src/cua.rs` and map it in `decode_action`.
2. Add the corresponding method on `Browser` if needed, keeping CDP-first design.
3. Handle the action in `Agent::handle_action`, including any necessary `wait_for_stable()`.
4. If the model requires a screenshot after the action, let the agent loop supply it (already supported).

## Testing & Validation
- There are no networked tests. Do not add tests that reach external services.
- If adding tests, prefer unit tests with small, pure functions or HTTP layer tests using mocked responses.
- Validate builds with `cargo build` and examples with `cargo run --example quickstart` (requires API key and network).

## Non-goals / Do Not
- Do not introduce breaking changes to the public API without explicit request.
- Do not add heavy transitive dependencies for convenience.
- Do not remove the Chromium profile isolation or hardcode OS-specific paths.
- Do not print secrets or include them in repository files.

## Troubleshooting
- Chromium profile lock errors: the unique `user-data-dir` should mitigate; ensure no zombie Chromium processes remain.
- `OPENAI_API_KEY missing`: set the env var before running examples.
- Model/tool field mismatches: `normalize_tools` adapts width/height keys; keep usage consistent.

---
This AGENTS.md guides contributors and automated agents working within this repo. Follow these practices for consistent, reliable changes.

