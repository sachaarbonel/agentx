use agentx::{Agent, AgentConfig, BrowserConfig};
use agentx::agent::{ChromiumComputer, CuaReasoner, DiskSnapshotStore};
use agentx::cua::{CuaClient, CuaConfig};
use anyhow::Result;
use std::time::Duration;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use serde_json;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let computer = if let Ok(ws) = std::env::var("CHROME_WS_URL") {
        if !ws.trim().is_empty() { ChromiumComputer::connect(&ws).await? } else { ChromiumComputer::launch(BrowserConfig { headless: false, user_agent: None }).await? }
    } else {
        ChromiumComputer::launch(BrowserConfig { headless: false, user_agent: None }).await?
    };
    let cua = CuaClient::new(CuaConfig { ..Default::default() })?;
    let reasoner = CuaReasoner::with_config(
        cua,
        "Proceed without asking for confirmations. Complete the task end-to-end.",
        agentx::agent::CuaReasonerConfig { stop_on_message: false, auto_confirm_text: Some("Yes, proceed and download the invoice PDF.".to_string()) }
    );
    let runs_dir = std::env::temp_dir().join("agentx_runs");
    let store = Arc::new(DiskSnapshotStore::new(runs_dir.clone()));
    let agent = Agent::with_defaults(computer, reasoner, AgentConfig { max_steps: 40, step_timeout: Duration::from_millis(3000), scopes: vec![] })
        .with_snapshot_store(store)
        .with_artifacts_dir(runs_dir.clone());

    // Single goal. The CUA model will ask for screenshots and issue actions.
    let report = agent.run(
        "Go to OpenAI Billing. Open the invoice labeled 'Paid $900.09 Aug 25, 2025'. Follow redirects in the same tab and download the PDF.",
        Some("https://platform.openai.com"),
    ).await?;

    Ok(())
}

