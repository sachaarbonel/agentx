use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use chromiumoxide::browser::Browser as OxideBrowser;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::layout::Point;
use chromiumoxide::page::{Page};
use futures::StreamExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

#[derive(Clone)]
pub struct BrowserConfig {
    pub headless: bool,
    pub user_agent: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self { headless: true, user_agent: None }
    }
}

pub struct Browser {
    page: Page,
    _browser: OxideBrowser,
}

impl Browser {
    pub async fn launch(cfg: BrowserConfig) -> Result<Self> {
        let mut builder = chromiumoxide::browser::BrowserConfig::builder();
        if !cfg.headless {
            builder = builder.with_head();
        }
        // Use a unique user data dir per run to avoid ProcessSingleton profile lock conflicts
        // observed when Chromium is restarted rapidly or multiple instances are spawned.
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        let mut profile_dir: PathBuf = std::env::temp_dir();
        profile_dir.push(format!("chromiumoxide-profile-{}-{}", std::process::id(), ts));
        let _ = std::fs::create_dir_all(&profile_dir);
        // Pass Chromium flags via builder to isolate profiles and reduce interruptions
        // Prefer explicit API if available; args remain as a fallback
        builder = builder.user_data_dir(profile_dir.clone());
        builder = builder
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check");
        let bcfg = builder.build().map_err(|e| anyhow::anyhow!(e))?;
        let (browser, mut handler) = OxideBrowser::launch(bcfg).await?;
        tokio::spawn(async move {
            while let Some(_ev) = handler.next().await {}
        });
        let page = browser.new_page("about:blank").await?;
        if let Some(ua) = cfg.user_agent {
            page.set_user_agent(ua).await?;
        }
        // Ensure a non-zero viewport to avoid screenshot 0-width errors
        let _ = page
            .execute(
                SetDeviceMetricsOverrideParams::builder()
                    .width(1280)
                    .height(800)
                    .device_scale_factor(1.0)
                    .mobile(false)
                    .build()
                    .unwrap(),
            )
            .await;
        // no SetVisibleSize in chromiumoxide 0.7; metrics override is enough
        Ok(Self { page, _browser: browser })
    }

    pub async fn goto(&self, url: &str) -> Result<()> {
        self.page.goto(url).await?;
        self.page.wait_for_navigation().await?;
        Ok(())
    }

    pub async fn url(&self) -> Result<String> {
        Ok(self.page.url().await?.unwrap_or_default())
    }

    pub async fn move_mouse(&self, x: i64, y: i64) -> Result<()> {
        self.page.move_mouse(Point { x: x as f64, y: y as f64 }).await?;
        Ok(())
    }

    pub async fn click(&self, x: i64, y: i64, button: &str) -> Result<()> {
        let btn = match button {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => MouseButton::Left,
        };
        // custom dispatch to honor button
        let cmd = DispatchMouseEventParams::builder()
            .x(x as f64)
            .y(y as f64)
            .button(btn)
            .click_count(1);
        self.page
            .move_mouse(Point { x: x as f64, y: y as f64 })
            .await?
            .execute(
                cmd.clone().r#type(DispatchMouseEventType::MousePressed).build().unwrap(),
            )
            .await?;
        self.page
            .execute(cmd.r#type(DispatchMouseEventType::MouseReleased).build().unwrap())
            .await?;
        Ok(())
    }

    pub async fn double_click(&self, x: i64, y: i64) -> Result<()> {
        let cmd = DispatchMouseEventParams::builder()
            .x(x as f64)
            .y(y as f64)
            .button(MouseButton::Left)
            .click_count(2);
        self.page
            .move_mouse(Point { x: x as f64, y: y as f64 })
            .await?
            .execute(
                cmd.clone().r#type(DispatchMouseEventType::MousePressed).build().unwrap(),
            )
            .await?;
        self.page
            .execute(cmd.r#type(DispatchMouseEventType::MouseReleased).build().unwrap())
            .await?;
        Ok(())
    }

    pub async fn scroll(&self, dx: i64, dy: i64) -> Result<()> {
        let script = format!("window.scrollBy({dx}, {dy});");
        let eval = EvaluateParams::builder()
            .expression(script)
            .build()
            .map_err(|e| anyhow::anyhow!(e))?;
        self.page.execute(eval).await?;
        Ok(())
    }

    pub async fn type_text(&self, text: &str) -> Result<()> {
        // Use CDP Input.insertText to feed active element
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
        self.page
            .execute(InsertTextParams { text: text.to_string() })
            .await?;
        Ok(())
    }

    pub async fn keypress(&self, key: &str) -> Result<()> {
        let k = key.to_string();
        let js = format!(r#"
            (function() {{
              const el = document.activeElement || document.body;
              const opts = {{key: "{k}", code: "{k}", bubbles: true}};
              el.dispatchEvent(new KeyboardEvent("keydown", opts));
              el.dispatchEvent(new KeyboardEvent("keyup", opts));
            }})()
        "#);
        let eval = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| anyhow::anyhow!(e))?;
        self.page.execute(eval).await?;
        Ok(())
    }

    pub async fn drag_path(&self, points: &[(i64, i64)]) -> Result<()> {
        if points.is_empty() { return Ok(()); }
        let (sx, sy) = points[0];
        let down = DispatchMouseEventParams::builder()
            .x(sx as f64).y(sy as f64).button(MouseButton::Left);
        self.page
            .move_mouse(Point { x: sx as f64, y: sy as f64 }).await?
            .execute(down.clone().r#type(DispatchMouseEventType::MousePressed).build().unwrap())
            .await?;
        for &(x, y) in &points[1..] {
            self.page
                .move_mouse(Point { x: x as f64, y: y as f64 })
                .await?;
        }
        self.page
            .execute(down.r#type(DispatchMouseEventType::MouseReleased).build().unwrap())
            .await?;
        Ok(())
    }

    pub async fn screenshot_b64(&self) -> Result<String> {
        use chromiumoxide::page::ScreenshotParamsBuilder;
        let take = || async {
            self
                .page
                .screenshot(
                    ScreenshotParamsBuilder::default()
                        .full_page(true)
                        .omit_background(true)
                        .build(),
                )
                .await
        };
        match take().await {
            Ok(bytes) => Ok(STANDARD.encode(bytes)),
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("0 width") || msg.contains("0 height") {
                    // Force viewport and retry once
                    let _ = self
                        .page
                        .execute(
                            SetDeviceMetricsOverrideParams::builder()
                                .width(1280)
                                .height(800)
                                .device_scale_factor(1.0)
                                .mobile(false)
                                .build()
                                .unwrap(),
                        )
                        .await;
                    sleep(Duration::from_millis(50)).await;
                    let bytes = take().await?;
                    return Ok(STANDARD.encode(bytes));
                }
                Err(anyhow::anyhow!(e))
            }
        }
    }

    pub async fn wait_for_stable(&self) -> Result<()> {
        sleep(Duration::from_millis(400)).await;
        Ok(())
    }
}

