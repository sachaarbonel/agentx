# Quickstart (glass-hands examples/quickstart)

This example runs a computer-use agent that opens Chromium/Chrome and completes a simple billing task on the OpenAI platform.

## Prerequisites

- Rust (stable, edition 2021)
- Chrome or Chromium on PATH
- OpenAI API key (Responses API + computer-use)

## Environment

```bash
export OPENAI_API_KEY=sk-...    # your key
export RUST_LOG=info            # optional, enables step-by-step logs
```

## Run (launch new Chromium)

```bash
cargo run --example quickstart
```

The example starts at `https://platform.openai.com` and pursues the goal embedded in `examples/quickstart.rs`. Snapshots go to a temp dir like `/tmp/glass_hands_runs/<run_id>/`.

## Reuse an existing Chrome session (CHROME_WS_URL)

Start Chrome with remote debugging and a dedicated profile:

```bash
nohup /Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/glass-hands-devtools \
  >/dev/null 2>&1 &
```

Get the DevTools WebSocket URL:

```bash
# With jq
curl -s http://127.0.0.1:9222/json/version | jq -r .webSocketDebuggerUrl

# Without jq
curl -s http://127.0.0.1:9222/json/version | python3 -c 'import sys,json; print(json.load(sys.stdin)["webSocketDebuggerUrl"])'
```

Run the example reusing that Chrome:

```bash
export CHROME_WS_URL=ws://127.0.0.1:9222/devtools/browser/<id>
export OPENAI_API_KEY=sk-...
RUST_LOG=info cargo run --example quickstart
```

When `CHROME_WS_URL` is set and non-empty, the example connects to that browser instead of launching a new one.

## What it does

- Sends a single goal to the model (configurable in `examples/quickstart.rs`)
- Lets the model request screenshots and issue actions via the hosted tool
- Keeps navigation in a single tab to follow redirects (e.g., Stripe)
- Logs each plan/action/result; saves screenshots per step

## Change the goal

Edit near the bottom of `examples/quickstart.rs`:

```rust
let _report = agent.run(
    "Go to OpenAI Billing. Open the invoice labeled 'Paid $900.09 Aug 25, 2025'. Follow redirects in the same tab and download the PDF.",
    Some("https://platform.openai.com"),
).await?;
```

## Notes

- `OPENAI_API_KEY` is required
- For existing Chrome, use `--remote-debugging-port` and non-default `--user-data-dir`
- Occasional CDP messages like "Failed to deserialize WS response" are benign
- A final "No tool output found for computer call" after success is safe to ignore
