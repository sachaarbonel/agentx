use agentx::{Agent, AgentConfig, Browser, BrowserConfig, CuaClient, CuaConfig};
use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // export OPENAI_API_KEY=sk-...
    let cua = CuaConfig { ..Default::default() };
    let browser = Browser::launch(BrowserConfig { headless: false, user_agent: None }).await?;

    let agent = Agent::new(
        cua,
        browser,
        AgentConfig { max_steps: 40, wait_after_nav_ms: 300 },
    )
    .await?;

    // Example goal. The CUA model will ask for screenshots and issue actions.
    agent
        .run("Open example.com. Click the More information link. Then stop.", Some("https://example.com"))
        .await?;

    Ok(())
}

