use crate::browser::Browser;
use crate::cua::{
    CuaAction, CuaClient, CuaConfig, CuaOutput, CuaToolImage, ResponseId, TurnInput,
};
use anyhow::Result;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

#[derive(Clone)]
pub struct AgentConfig {
    pub max_steps: usize,
    pub wait_after_nav_ms: u64,
}

pub struct Agent {
    cua: CuaClient,
    browser: Browser,
    cfg: AgentConfig,
}

impl Agent {
    pub async fn new(cua_cfg: CuaConfig, browser: Browser, cfg: AgentConfig) -> Result<Self> {
        Ok(Self {
            cua: CuaClient::new(cua_cfg)?,
            browser,
            cfg,
        })
    }

    pub async fn run(&self, goal: &str, start_url: Option<&str>) -> Result<()> {
        if let Some(url) = start_url {
            self.browser.goto(url).await?;
            self.browser.wait_for_stable().await?;
        }
        let mut prev: Option<ResponseId> = None;

        for step in 0..self.cfg.max_steps {
            let current_url = self.browser.url().await.unwrap_or_default();
            let input = TurnInput {
                instructions: goal.to_owned(),
                current_url: Some(current_url),
                ..Default::default()
            };

            let mut out = self.cua.turn(input, prev.as_ref()).await?;
            loop {
                match out {
                    CuaOutput::Message { text } => {
                        info!(%step, "agent message: {}", text.trim());
                        return Ok(());
                    }
                    CuaOutput::ComputerCall {
                        call_id,
                        action,
                        requires_screenshot,
                        response_id,
                        safety_checks,
                    } => {
                        self.handle_action(&action).await?;
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        let png_b64 = self.browser.screenshot_b64().await?;
                        let _ = requires_screenshot;
                        let next = self
                            .cua
                            .send_computer_output(
                                &call_id,
                                CuaToolImage {
                                    r#type: "input_image".into(),
                                    mime_type: "image/png".into(),
                                    data_base64: png_b64,
                                },
                                Some(&response_id),
                                Some(&safety_checks),
                            )
                            .await?;
                        prev = Some(response_id);
                        out = next;
                        // Continue chaining tool calls without creating a new turn
                        continue;
                    }
                    CuaOutput::Done { response_id } => {
                        info!(%step, "done: {}", response_id.0);
                        return Ok(());
                    }
                }
                break;
            }

            sleep(Duration::from_millis(self.cfg.wait_after_nav_ms)).await;
        }
        Ok(())
    }

    async fn handle_action(&self, action: &CuaAction) -> Result<()> {
        use CuaAction::*;
        match action {
            Screenshot => { /* screenshot handled in caller */ }
            Click { x, y, button } => {
                let btn = button.as_deref().unwrap_or("left");
                self.browser.click(*x, *y, btn).await?;
                self.browser.wait_for_stable().await?;
            }
            DoubleClick { x, y } => {
                self.browser.double_click(*x, *y).await?;
                self.browser.wait_for_stable().await?;
            }
            Move { x, y } => {
                self.browser.move_mouse(*x, *y).await?;
            }
            Scroll { dx, dy } => {
                self.browser.scroll(*dx, *dy).await?;
            }
            Type { text } => {
                self.browser.type_text(text).await?;
            }
            Keypress { key } => {
                self.browser.keypress(key).await?;
                self.browser.wait_for_stable().await?;
            }
            DragPath { points } => {
                self.browser.drag_path(points).await?;
                self.browser.wait_for_stable().await?;
            }
            WaitMs { ms } => {
                tokio::time::sleep(Duration::from_millis((*ms).max(0) as u64)).await;
            }
            Unknown(v) => {
                warn!("unknown action: {}", v);
            }
        }
        Ok(())
    }
}

