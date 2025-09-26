use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;

#[derive(Clone)]
pub struct CuaConfig {
    pub api_base: String,      // e.g. "https://api.openai.com/v1"
    pub api_key: String,       // env OPENAI_API_KEY
    pub model: String,         // e.g. "computer-use-preview"
    pub tool_display: (u32, u32),
    pub environment: String,   // "browser"
}

impl Default for CuaConfig {
    fn default() -> Self {
        Self {
            api_base: env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: env::var("OPENAI_CUA_MODEL").unwrap_or_else(|_| "computer-use-preview".into()),
            tool_display: (1280, 800),
            environment: "browser".into(),
        }
    }
}

#[derive(Clone)]
pub struct CuaClient {
    http: Client,
    cfg: CuaConfig,
}

#[derive(Clone, Debug)]
pub struct ResponseId(pub String);

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TurnInput {
    pub instructions: String,
    pub current_url: Option<String>,
}

#[derive(Debug)]
pub enum CuaOutput {
    Message { text: String },
    ComputerCall {
        call_id: String,
        action: CuaAction,
        requires_screenshot: bool,
        response_id: ResponseId,
        safety_checks: Vec<Value>,
    },
    Done { response_id: ResponseId },
}

#[derive(Debug, Clone)]
pub enum CuaAction {
    Screenshot,
    Click { x: i64, y: i64, button: Option<String> },
    DoubleClick { x: i64, y: i64 },
    Move { x: i64, y: i64 },
    Scroll { dx: i64, dy: i64 },
    Type { text: String },
    Keypress { key: String },
    DragPath { points: Vec<(i64, i64)> },
    WaitMs { ms: i64 },
    Unknown(String),
}

#[derive(Debug, Serialize)]
pub struct CuaToolImage {
    pub r#type: String,      // "input_image"
    pub mime_type: String,   // "image/png"
    #[serde(rename = "data")]
    pub data_base64: String, // base64 png
}

impl CuaClient {
    pub fn new(cfg: CuaConfig) -> Result<Self> {
        if cfg.api_key.is_empty() {
            bail!("OPENAI_API_KEY missing");
        }
        Ok(Self {
            http: Client::new(),
            cfg,
        })
    }

    pub async fn turn(&self, input: TurnInput, previous: Option<&ResponseId>) -> Result<CuaOutput> {
        let url = format!("{}/responses", self.cfg.api_base);
        let mut req = json!({
          "model": self.cfg.model,
          "truncation": "auto",
          "input": [
            { "role": "user", "content": [
                { "type": "input_text", "text": input.instructions },
                { "type": "input_text", "text": format!("current_url={}", input.current_url.unwrap_or_default()) }
            ]}
          ]
        });

        // Include the hosted computer use tool only for computer-use models
        let wants_computer_tool = self.cfg.model.contains("computer-use");
        if wants_computer_tool {
            req["tools"] = json!([{
                "type": "computer_use_preview",
                "display_width_px": self.cfg.tool_display.0,
                "display_height_px": self.cfg.tool_display.1,
                "environment": self.cfg.environment
            }]);
        }
        if let Some(prev) = previous {
            req["previous_response_id"] = Value::String(prev.0.clone());
        }
        // Note: For Zero Data Retention orgs, previous_response_id is not supported.

        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .json(&Self::normalize_tools(req))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("OpenAI error {}: {}", status, text);
        }
        let v: Value = serde_json::from_str(&text).context("failed to parse OpenAI response JSON")?;
        Self::parse_output(v)
    }

    pub async fn send_computer_output(
        &self,
        call_id: &str,
        image: CuaToolImage,
        _previous: Option<&ResponseId>,
        acknowledged_safety_checks: Option<&[Value]>,
    ) -> Result<CuaOutput> {
        let url = format!("{}/responses", self.cfg.api_base);
        let mut req = json!({
          "model": self.cfg.model,
          "truncation": "auto",
          "input": [{
            "type": "computer_call_output",
            "call_id": call_id,
            "output": {
              "type": "input_image",
              "image_url": format!("data:{};base64,{}", image.mime_type, image.data_base64)
            },
            "acknowledged_safety_checks": acknowledged_safety_checks
          }]
        });
        // Ensure the hosted tool is enabled when sending computer output
        if self.cfg.model.contains("computer-use") {
            req["tools"] = json!([{
                "type": "computer_use_preview",
                "display_width": self.cfg.tool_display.0,
                "display_height": self.cfg.tool_display.1,
                "environment": self.cfg.environment
            }]);
        }
        if let Some(prev) = _previous {
            // Non-ZDR orgs: continue the response thread
            req["previous_response_id"] = Value::String(prev.0.clone());
        }
        // Do not include previous_response_id to support Zero Data Retention orgs

        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cfg.api_key)
            .json(&Self::normalize_tools(req))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("OpenAI error {}: {}", status, text);
        }
        let v: Value = serde_json::from_str(&text).context("failed to parse OpenAI response JSON")?;
        Self::parse_output(v)
    }

    fn parse_output(v: Value) -> Result<CuaOutput> {
        // The Responses API returns: { id, output: [ ... ], status }
        let response_id = v
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| ResponseId(s.to_string()))
            .context("missing id")?;

        let outputs = v
            .get("output")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();

        // Prioritize handling of computer_call over message per Responses API contract
        let mut pending_message: Option<String> = None;
        for o in &outputs {
            if let Some(t) = o.get("type").and_then(|x| x.as_str()) {
                if t == "computer_call" {
                    let call_id = o
                        .get("call_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();

                    let requires_screenshot = o
                        .get("requires_screenshot")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(true);

                    let action = o
                        .get("action")
                        .cloned()
                        .map(Self::decode_action)
                        .transpose()?
                        .unwrap_or(CuaAction::Unknown("unknown".into()));

                    let safety_checks = o
                        .get("pending_safety_checks")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();

                    return Ok(CuaOutput::ComputerCall {
                        call_id,
                        action,
                        requires_screenshot,
                        response_id,
                        safety_checks,
                    });
                }
                if t == "message" {
                    if let Some(text) = o.pointer("/content/0/text").and_then(|x| x.as_str()) {
                        pending_message = Some(text.to_string());
                    }
                } else if t == "done" {
                    return Ok(CuaOutput::Done { response_id });
                }
            }
        }

        if let Some(text) = pending_message {
            return Ok(CuaOutput::Message { text });
        }

        // Fallback
        Ok(CuaOutput::Done { response_id })
    }

    fn normalize_tools(mut v: Value) -> Value {
        // OpenAI Responses API expects tool fields display_width/display_height
        // instead of display_width_px/display_height_px in some deployments.
        // Convert if necessary.
        if let Some(tools) = v.get_mut("tools").and_then(|x| x.as_array_mut()) {
            for t in tools.iter_mut() {
                if let Some(obj) = t.as_object_mut() {
                    if let Some(wpx) = obj.remove("display_width_px") {
                        obj.insert("display_width".to_string(), wpx);
                    }
                    if let Some(hpx) = obj.remove("display_height_px") {
                        obj.insert("display_height".to_string(), hpx);
                    }
                }
            }
        }
        v
    }

    fn decode_action(v: Value) -> Result<CuaAction> {
        let kind = v
            .get("type")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string();
        let a = match kind.as_str() {
            "screenshot" => CuaAction::Screenshot,
            "click" => CuaAction::Click {
                x: v.get("x").and_then(|x| x.as_i64()).unwrap_or(0),
                y: v.get("y").and_then(|x| x.as_i64()).unwrap_or(0),
                button: v.get("button").and_then(|x| x.as_str()).map(|s| s.to_string()),
            },
            "double_click" => CuaAction::DoubleClick {
                x: v.get("x").and_then(|x| x.as_i64()).unwrap_or(0),
                y: v.get("y").and_then(|x| x.as_i64()).unwrap_or(0),
            },
            "move" => CuaAction::Move {
                x: v.get("x").and_then(|x| x.as_i64()).unwrap_or(0),
                y: v.get("y").and_then(|x| x.as_i64()).unwrap_or(0),
            },
            "scroll" => CuaAction::Scroll {
                dx: v.get("x").or_else(|| v.get("dx")).and_then(|x| x.as_i64()).unwrap_or(0),
                dy: v.get("y").or_else(|| v.get("dy")).and_then(|x| x.as_i64()).unwrap_or(0),
            },
            "type" => CuaAction::Type {
                text: v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            },
            "keypress" => CuaAction::Keypress {
                key: v.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            },
            "drag" | "drag_path" => {
                let points = v
                    .get("points")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| {
                                let x = p.get("x")?.as_i64()?;
                                let y = p.get("y")?.as_i64()?;
                                Some((x, y))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                CuaAction::DragPath { points }
            }
            "wait" | "wait_ms" => CuaAction::WaitMs {
                ms: v.get("ms").and_then(|x| x.as_i64()).unwrap_or(300),
            },
            _ => CuaAction::Unknown(kind),
        };
        Ok(a)
    }
}

