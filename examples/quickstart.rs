use agentx::{Agent, AgentConfig, BrowserConfig};
use agentx::agent::{ChromiumComputer, CuaReasoner, DiskSnapshotStore};
use agentx::cua::{CuaClient, CuaConfig};
use anyhow::Result;
use std::time::Duration;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let computer = ChromiumComputer::launch(BrowserConfig { headless: false, user_agent: None }).await?;
    let cua = CuaClient::new(CuaConfig { ..Default::default() })?;
    let reasoner = CuaReasoner::new(cua, "Open example.com. Click the More information link. Then stop.");
    let store = Arc::new(DiskSnapshotStore::new(std::env::temp_dir().join("agentx_runs")));
    let agent = Agent::with_defaults(computer, reasoner, AgentConfig { max_steps: 40, step_timeout: Duration::from_millis(3000), scopes: vec![] })
        .with_snapshot_store(store);

    // Example goal. The CUA model will ask for screenshots and issue actions.
    let _report = agent.run("", Some("https://example.com")).await?;

    Ok(())
}

