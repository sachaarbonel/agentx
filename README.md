# agentx

Build browser-native computer-use agents in Rust. Agentx connects OpenAI computer-use (Responses API) to a real Chromium instance via CDP, turning plain-text goals into deterministic browser actions.

## Get started in 30 seconds
1) Requirements
- Rust (stable, edition 2021)
- Chrome or Chromium on your PATH
- OpenAI API key

2) Configure
```bash
export OPENAI_API_KEY=sk-...
export RUST_LOG=info
```

3) Run
```bash
cargo run --example quickstart
```
You’ll see Chromium open and the agent execute a tiny task on `example.com`.

## Configure
Environment variables:
- `OPENAI_API_KEY` (required)
- `OPENAI_BASE_URL` (optional, default `https://api.openai.com/v1`)
- `OPENAI_CUA_MODEL` (optional, default `computer-use-preview`)

Tune at runtime via code:
- `BrowserConfig` (e.g., headless vs interactive, user agent)
- `AgentConfig` (e.g., `max_steps`, `step_timeout`, `scopes`)

## Use it in your app
See a complete, minimal program in `examples/quickstart.rs`. It shows how to:
- Launch a Chromium-powered computer (`ChromiumComputer`)
- Create a CUA client (`CuaClient` with `CuaConfig`)
- Build a reasoner (`CuaReasoner`) from plain-text instructions
- Run an agent and optionally persist snapshots (`DiskSnapshotStore`)


## Troubleshooting
- Set `OPENAI_API_KEY` before running
- If Chromium profile lock errors occur, ensure no zombie Chrome processes remain (agentx uses an isolated user-data-dir per run)
- Width/height tool field differences are normalized internally

## Security
- Don’t log or commit secrets
- Avoid embedding API keys or raw auth headers in code or logs

## License
No explicit license is included. If you need clarification on usage rights, please open an issue.
