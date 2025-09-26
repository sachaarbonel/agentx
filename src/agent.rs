use async_trait::async_trait;
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{info, warn};
use crate::browser::Browser;
use crate::cua::{CuaAction, CuaClient, CuaOutput, CuaToolImage, ResponseId};
use serde_json::Value;
use tokio::sync::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs as async_fs;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

// ========================= Core Types =========================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Click { target: Locator },
    Type { text: String, into: Locator },
    Key { combo: String },
    Hover { target: Locator },
    Scroll { target: Option<Locator>, dx: i32, dy: i32 },
    Drag { from: Locator, to: Locator },
    NavGoto { url: String },
    Submit { target: Locator },
    FileUpload { target: Locator, path: String },
    ClipboardRead,
    ClipboardWrite { data: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Locator {
    Css { selector: String },
    XPath { expr: String },
    Text { pattern: String },
    Id { id: String },
    Aria { role: Option<String>, name: Option<String> },
    Coordinates { x: i32, y: i32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomNode {
    pub locator: Locator,
    pub description: Option<String>,
    pub rect: Option<DomRect>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub url: Option<String>,
    pub title: Option<String>,
    pub image_base64: Option<String>,
    pub dom_summary: Option<String>,
    pub captured_at_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionResult {
    pub snapshot: Snapshot,
    pub changed: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Memory {
    pub run_id: String,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Goal {
    pub task: String,
    pub constraints: Vec<String>,
    pub success_criteria: Vec<String>,
    // wall-clock deadline is not serializable; use relative timeout budget instead
    pub timeout_ms: Option<u128>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thought {
    pub plan: String,
    pub action: Option<Action>,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Scope {
    BrowserNavigate,
    ClipboardRead,
    ClipboardWrite,
    FileAccess,
    Network,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub granted: bool,
    pub scope: Option<Scope>,
    pub reason: Option<String>,
}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum AgentError {
    #[error("computer error: {0}")]
    Computer(String),
    #[error("reasoner error: {0}")]
    Reasoner(String),
    #[error("policy denied: {0:?}")]
    Denied(Scope),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("memory error: {0}")]
    Memory(String),
    #[error("other error: {0}")]
    Other(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RunStatus {
    Success,
    Timeout,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RunMetrics {
    pub steps: usize,
    pub time_ms: u128,
    pub success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StepLog {
    pub step: usize,
    pub plan: String,
    pub action: Option<Action>,
    pub approval: Option<Approval>,
    pub result_hint: String,
    pub snapshot_id: Option<String>,
    pub error: Option<String>,
    pub timestamp_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub goal: Goal,
    pub status: RunStatus,
    pub metrics: RunMetrics,
    pub steps: Vec<StepLog>,
    pub last_snapshot: Option<Snapshot>,
    pub error: Option<String>,
}

// ========================= Pluggable Subsystems =========================

#[async_trait]
pub trait Computer: Send + Sync {
    async fn open_url(&self, url: &str) -> Result<Snapshot, AgentError>;
    async fn snapshot(&self) -> Result<Snapshot, AgentError>;
    async fn find(&self, locator: &Locator, timeout: Duration) -> Result<DomNode, AgentError>;
    async fn act(&self, action: &Action, timeout: Duration) -> Result<ActionResult, AgentError>;
}

#[async_trait]
pub trait Reasoner: Send + Sync {
    async fn think(
        &self,
        goal: &Goal,
        memory: &Memory,
        snapshot: &Snapshot,
        last_error: Option<&AgentError>,
    ) -> Result<Thought, AgentError>;

    async fn success(
        &self,
        goal: &Goal,
        snapshot: &Snapshot,
        memory: &Memory,
    ) -> Result<bool, AgentError>;
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn write_run_start(&self, run_id: &str, goal: &Goal) -> Result<(), AgentError>;
    async fn write_step(&self, run_id: &str, step: &StepLog) -> Result<(), AgentError>;
    async fn write_run_end(&self, run_id: &str, report: &RunReport) -> Result<(), AgentError>;
}

#[async_trait]
pub trait SnapshotStore: Send + Sync {
    async fn save(&self, run_id: &str, step: Option<usize>, snapshot: &Snapshot) -> Result<(), AgentError>;
}

#[async_trait]
pub trait PolicyEngine: Send + Sync {
    async fn approve(&self, scopes: &[Scope], action: &Action) -> Result<Approval, AgentError>;
}

// ========================= Agent Core =========================

#[derive(Clone)]
pub struct AgentConfig {
    pub max_steps: usize,
    pub step_timeout: Duration,
    pub scopes: Vec<Scope>,
}

pub struct Agent<C, R, M, P>
where
    C: Computer,
    R: Reasoner,
    M: MemoryStore,
    P: PolicyEngine,
{
    computer: C,
    reasoner: R,
    memory: M,
    policy: P,
    cfg: AgentConfig,
    snapshot_store: Option<Arc<dyn SnapshotStore>>, // optional sink for snapshots
}

impl<C, R, M, P> Agent<C, R, M, P>
where
    C: Computer,
    R: Reasoner,
    M: MemoryStore,
    P: PolicyEngine,
{
    pub fn new(computer: C, reasoner: R, memory: M, policy: P, cfg: AgentConfig) -> Self {
        Self {
            computer,
            reasoner,
            memory,
            policy,
            cfg,
            snapshot_store: None,
        }
    }

    pub async fn run(&self, goal: &str, start_url: Option<&str>) -> Result<RunReport, AgentError> {
        let goal = Goal {
            task: goal.to_string(),
            constraints: vec![],
            success_criteria: vec![],
            timeout_ms: None,
        };
        self.run_goal(goal, start_url).await
    }

    pub async fn run_goal(
        &self,
        goal: Goal,
        start_url: Option<&str>,
    ) -> Result<RunReport, AgentError> {
        let run_id = nanoid!();
        let start = Instant::now();
        let mut metrics = RunMetrics::default();
        let mut steps: Vec<StepLog> = Vec::new();
        let mut last_error: Option<AgentError> = None;

        self.memory.write_run_start(&run_id, &goal).await?;

        let mut last_snapshot = match start_url {
            Some(url) => self.computer.open_url(url).await?,
            None => self.computer.snapshot().await?,
        };
        if let Some(store) = &self.snapshot_store {
            let _ = store.save(&run_id, None, &last_snapshot).await;
        }

        let memory = Memory {
            run_id: run_id.clone(),
            notes: Vec::new(),
        };

        let deadline = goal.timeout_ms.map(|ms| start + Duration::from_millis(ms as u64));

        for i in 0..self.cfg.max_steps {
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    return self
                        .finish(
                            run_id,
                            goal,
                            steps,
                            metrics,
                            last_snapshot,
                            RunStatus::Timeout,
                            "Run budget exceeded",
                            None,
                        )
                        .await;
                }
            }

            let success = self
                .reasoner
                .success(&goal, &last_snapshot, &memory)
                .await?;
            if success {
                metrics.success = true;
                metrics.steps = i;
                metrics.time_ms = start.elapsed().as_millis();
                return self
                    .finish(
                        run_id,
                        goal,
                        steps,
                        metrics,
                        last_snapshot,
                        RunStatus::Success,
                        "Goal met",
                        None,
                    )
                    .await;
            }

            let thought = self
                .reasoner
                .think(&goal, &memory, &last_snapshot, last_error.as_ref())
                .await?;
            let maybe_action = thought.action.clone();
            let mut step_log = StepLog {
                step: i,
                plan: thought.plan.clone(),
                action: maybe_action.clone(),
                approval: None,
                result_hint: String::new(),
                snapshot_id: None,
                error: None,
                timestamp_ms: Instant::now().duration_since(start).as_millis(),
            };
            info!(step = i, plan = %thought.plan, has_action = %maybe_action.is_some(), "agent step");

            if maybe_action.is_none() && !thought.plan.trim().is_empty() {
                info!(step = i, "agent message: {}", thought.plan.trim());
                step_log.result_hint = "message".into();
                self.memory.write_step(&run_id, &step_log).await?;
                steps.push(step_log);
                continue;
            }

            if let Some(action) = &maybe_action {
                let approval = self.policy.approve(&self.cfg.scopes, action).await?;
                step_log.approval = Some(approval.clone());
                if !approval.granted {
                    last_error = Some(AgentError::Denied(
                        approval.scope.unwrap_or(Scope::BrowserNavigate),
                    ));
                    step_log.result_hint = "denied".into();
                    self.memory.write_step(&run_id, &step_log).await?;
                    steps.push(step_log);
                    info!(step = i, "action denied by policy");
                    continue;
                }
                info!(step = i, action = ?action, "action approved");
            }

            let result = if let Some(action) = maybe_action {
                self.computer.act(&action, self.cfg.step_timeout).await
            } else {
                Ok(ActionResult {
                    snapshot: self.computer.snapshot().await?,
                    changed: false,
                    message: Some("think".to_string()),
                })
            };

            match result {
                Ok(out) => {
                    last_snapshot = out.snapshot.clone();
                    if let Some(store) = &self.snapshot_store {
                        let _ = store.save(&memory.run_id, Some(i), &last_snapshot).await;
                    }
                    step_log.result_hint = if out.changed {
                        "changed".into()
                    } else {
                        "unchanged".into()
                    };
                    step_log.snapshot_id = Some(last_snapshot.id.clone());
                    last_error = None;
                    self.memory.write_step(&run_id, &step_log).await?;
                    steps.push(step_log);
                    info!(step = i, result = %"ok", changed = out.changed, url = ?last_snapshot.url, "action result");
                }
                Err(err) => {
                    warn!("step {} failed: {}", i, err);
                    step_log.error = Some(format!("{}", err));
                    step_log.result_hint = "error".into();
                    self.memory.write_step(&run_id, &step_log).await?;
                    steps.push(step_log);
                    last_error = Some(err);
                }
            }
        }

        metrics.success = false;
        metrics.steps = self.cfg.max_steps;
        metrics.time_ms = start.elapsed().as_millis();
        self
            .finish(
                run_id,
                goal,
                steps,
                metrics,
                last_snapshot,
                RunStatus::Timeout,
                "Step budget exceeded",
                last_error.map(|e| format!("{}", e)),
            )
            .await
    }

    async fn finish(
        &self,
        run_id: String,
        goal: Goal,
        steps: Vec<StepLog>,
        metrics: RunMetrics,
        last_snapshot: Snapshot,
        status: RunStatus,
        msg: &str,
        err: Option<String>,
    ) -> Result<RunReport, AgentError> {
        let report = RunReport {
            run_id: run_id.clone(),
            goal,
            status,
            metrics,
            steps,
            last_snapshot: Some(last_snapshot),
            error: err.or_else(|| Some(msg.to_string())),
        };
        self.memory.write_run_end(&run_id, &report).await?;
        info!("run {} finished", run_id);
        Ok(report)
    }
}

// ========================= Defaults & Helpers =========================

pub struct NullMemoryStore;

#[async_trait]
impl MemoryStore for NullMemoryStore {
    async fn write_run_start(&self, _run_id: &str, _goal: &Goal) -> Result<(), AgentError> {
        Ok(())
    }

    async fn write_step(&self, _run_id: &str, _step: &StepLog) -> Result<(), AgentError> {
        Ok(())
    }

    async fn write_run_end(&self, _run_id: &str, _report: &RunReport) -> Result<(), AgentError> {
        Ok(())
    }
}

pub struct DiskSnapshotStore {
    base_dir: PathBuf,
}

impl DiskSnapshotStore {
    pub fn new<P: AsRef<Path>>(base: P) -> Self {
        Self { base_dir: base.as_ref().to_path_buf() }
    }
}

#[async_trait]
impl SnapshotStore for DiskSnapshotStore {
    async fn save(&self, run_id: &str, step: Option<usize>, snapshot: &Snapshot) -> Result<(), AgentError> {
        let dir = self.base_dir.join(run_id);
        async_fs::create_dir_all(&dir)
            .await
            .map_err(|e| AgentError::Memory(format!("create_dir: {}", e)))?;
        if let Some(b64) = &snapshot.image_base64 {
            let png = B64
                .decode(b64)
                .map_err(|e| AgentError::Memory(format!("b64 decode: {}", e)))?;
            let name = match step {
                Some(s) => format!("step_{:03}.png", s),
                None => "start.png".to_string(),
            };
            let path = dir.join(name);
            async_fs::write(&path, &png)
                .await
                .map_err(|e| AgentError::Memory(format!("write: {}", e)))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct AllowAllPolicy;

#[async_trait]
impl PolicyEngine for AllowAllPolicy {
    async fn approve(&self, _scopes: &[Scope], _action: &Action) -> Result<Approval, AgentError> {
        Ok(Approval { granted: true, scope: None, reason: Some("allow all".to_string()) })
    }
}

#[derive(Clone, Copy)]
pub struct NoopComputer;

#[async_trait]
impl Computer for NoopComputer {
    async fn open_url(&self, url: &str) -> Result<Snapshot, AgentError> {
        Ok(Snapshot {
            id: nanoid!(),
            url: Some(url.to_string()),
            title: Some("noop".to_string()),
            image_base64: None,
            dom_summary: Some("<noop/>".to_string()),
            captured_at_ms: 0,
        })
    }

    async fn snapshot(&self) -> Result<Snapshot, AgentError> {
        Ok(Snapshot {
            id: nanoid!(),
            url: Some("about:blank".to_string()),
            title: Some("noop".to_string()),
            image_base64: None,
            dom_summary: Some("<noop/>".to_string()),
            captured_at_ms: 0,
        })
    }

    async fn find(&self, locator: &Locator, _timeout: Duration) -> Result<DomNode, AgentError> {
        Ok(DomNode { locator: locator.clone(), description: Some("noop".to_string()), rect: Some(DomRect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 }) })
    }

    async fn act(&self, _action: &Action, _timeout: Duration) -> Result<ActionResult, AgentError> {
        let snap = self.snapshot().await?;
        Ok(ActionResult { snapshot: snap, changed: true, message: Some("noop".to_string()) })
    }
}

#[derive(Clone, Copy)]
pub struct SimpleReasoner;

#[async_trait]
impl Reasoner for SimpleReasoner {
    async fn think(
        &self,
        goal: &Goal,
        _memory: &Memory,
        _snapshot: &Snapshot,
        _last_error: Option<&AgentError>,
    ) -> Result<Thought, AgentError> {
        Ok(Thought { plan: format!("Plan: {}", goal.task), action: None, rationale: Some("noop".to_string()) })
    }

    async fn success(
        &self,
        goal: &Goal,
        _snapshot: &Snapshot,
        _memory: &Memory,
    ) -> Result<bool, AgentError> {
        Ok(goal.task.to_lowercase().contains("stop"))
    }
}

impl<C: Computer, R: Reasoner> Agent<C, R, NullMemoryStore, AllowAllPolicy> {
    pub fn with_defaults(computer: C, reasoner: R, cfg: AgentConfig) -> Self {
        Self::new(computer, reasoner, NullMemoryStore, AllowAllPolicy, cfg)
    }

    pub fn with_snapshot_store(mut self, store: Arc<dyn SnapshotStore>) -> Self {
        self.snapshot_store = Some(store);
        self
    }
}

// ========================= Chromium Adapter =========================

pub struct ChromiumComputer {
    browser: Browser,
}

impl ChromiumComputer {
    pub async fn launch(cfg: crate::browser::BrowserConfig) -> Result<Self, AgentError> {
        let browser = Browser::launch(cfg)
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        Ok(Self { browser })
    }

    pub async fn connect(ws_url: &str) -> Result<Self, AgentError> {
        let browser = Browser::connect(ws_url)
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        Ok(Self { browser })
    }
}

#[async_trait]
impl Computer for ChromiumComputer {
    async fn open_url(&self, url: &str) -> Result<Snapshot, AgentError> {
        self.browser
            .goto(url)
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        // Ensure links open in same tab to keep control
        let _ = self.browser.enable_single_tab_mode().await;
        self.browser
            .wait_for_stable()
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        let snap_b64 = self
            .browser
            .screenshot_b64()
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        Ok(Snapshot {
            id: nanoid!(),
            url: Some(url.to_string()),
            title: None,
            image_base64: Some(snap_b64),
            dom_summary: None,
            captured_at_ms: 0,
        })
    }

    async fn snapshot(&self) -> Result<Snapshot, AgentError> {
        let url = self
            .browser
            .url()
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        let snap_b64 = self
            .browser
            .screenshot_b64()
            .await
            .map_err(|e| AgentError::Other(e.to_string()))?;
        Ok(Snapshot {
            id: nanoid!(),
            url: Some(url),
            title: None,
            image_base64: Some(snap_b64),
            dom_summary: None,
            captured_at_ms: 0,
        })
    }

    async fn find(&self, locator: &Locator, _timeout: Duration) -> Result<DomNode, AgentError> {
        Ok(DomNode {
            locator: locator.clone(),
            description: Some("chromium".to_string()),
            rect: None,
        })
    }

    async fn act(&self, action: &Action, _timeout: Duration) -> Result<ActionResult, AgentError> {
        match action {
            Action::NavGoto { url } => {
                let _ = self.open_url(url).await?;
            }
            Action::Click { target } => {
                match target {
                    Locator::Coordinates { x, y } => {
                        self.browser
                            .click(*x as i64, *y as i64, "left")
                            .await
                            .map_err(|e| AgentError::Other(e.to_string()))?;
                    }
                    _ => {
                        return Err(AgentError::Other(
                            "click target type not implemented".into(),
                        ));
                    }
                }
            }
            Action::Hover { target } => {
                match target {
                    Locator::Coordinates { x, y } => {
                        self.browser
                            .move_mouse(*x as i64, *y as i64)
                            .await
                            .map_err(|e| AgentError::Other(e.to_string()))?;
                    }
                    _ => {
                        return Err(AgentError::Other(
                            "hover target type not implemented".into(),
                        ));
                    }
                }
            }
            Action::Scroll { target: None, dx, dy } => {
                self.browser
                    .scroll(*dx as i64, *dy as i64)
                    .await
                    .map_err(|e| AgentError::Other(e.to_string()))?;
            }
            Action::Key { combo } => {
                self.browser
                    .keypress(combo)
                    .await
                    .map_err(|e| AgentError::Other(e.to_string()))?;
            }
            Action::Type { text, .. } => {
                self.browser
                    .type_text(text)
                    .await
                    .map_err(|e| AgentError::Other(e.to_string()))?;
            }
            _ => {
                return Err(AgentError::Other(
                    "action not implemented in chromium adapter".into(),
                ));
            }
        }
        // Keep to same tab post-action as actions might trigger new tabs
        let _ = self.browser.enable_single_tab_mode().await;
        Ok(ActionResult {
            snapshot: self.snapshot().await?,
            changed: true,
            message: None,
        })
    }
}

// ========================= CUA-backed Reasoner =========================

struct CuaState {
    previous: Option<ResponseId>,
    pending_call_id: Option<String>,
    pending_safety_checks: Vec<Value>,
    awaiting_screenshot: bool,
    done_message: Option<String>,
}

impl Default for CuaState {
    fn default() -> Self {
        Self {
            previous: None,
            pending_call_id: None,
            pending_safety_checks: Vec::new(),
            awaiting_screenshot: false,
            done_message: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CuaReasonerConfig {
    pub stop_on_message: bool,
    pub auto_confirm_text: Option<String>,
}

impl Default for CuaReasonerConfig {
    fn default() -> Self {
        Self { stop_on_message: true, auto_confirm_text: None }
    }
}

#[derive(Clone)]
pub struct CuaReasoner {
    client: CuaClient,
    instructions: String,
    state: std::sync::Arc<Mutex<CuaState>>,
    cfg: CuaReasonerConfig,
}

impl CuaReasoner {
    pub fn new(client: CuaClient, instructions: impl Into<String>) -> Self {
        Self { client, instructions: instructions.into(), state: std::sync::Arc::new(Mutex::new(CuaState::default())), cfg: CuaReasonerConfig::default() }
    }

    pub fn with_config(client: CuaClient, instructions: impl Into<String>, cfg: CuaReasonerConfig) -> Self {
        Self { client, instructions: instructions.into(), state: std::sync::Arc::new(Mutex::new(CuaState::default())), cfg }
    }

    fn compose_instructions(base: &str, goal: &Goal) -> String {
        let mut s = String::new();
        if !base.trim().is_empty() {
            s.push_str(base);
            s.push_str("\n\n");
        }
        s.push_str("Goal: ");
        s.push_str(&goal.task);
        if !goal.constraints.is_empty() {
            s.push_str("\nConstraints:\n");
            for c in &goal.constraints {
                s.push_str("- ");
                s.push_str(c);
                s.push('\n');
            }
        }
        if !goal.success_criteria.is_empty() {
            s.push_str("Success criteria:\n");
            for c in &goal.success_criteria {
                s.push_str("- ");
                s.push_str(c);
                s.push('\n');
            }
        }
        s
    }

    fn map_cua_action(action: CuaAction) -> Option<Action> {
        match action {
            CuaAction::Click { x, y, .. } => Some(Action::Click { target: Locator::Coordinates { x: x as i32, y: y as i32 } }),
            CuaAction::DoubleClick { x, y } => Some(Action::Click { target: Locator::Coordinates { x: x as i32, y: y as i32 } }),
            CuaAction::Move { x, y } => Some(Action::Hover { target: Locator::Coordinates { x: x as i32, y: y as i32 } }),
            CuaAction::Scroll { dx, dy } => Some(Action::Scroll { target: None, dx: dx as i32, dy: dy as i32 }),
            CuaAction::Type { text } => Some(Action::Type { text, into: Locator::Css { selector: "*".to_string() } }),
            CuaAction::Keypress { key } => Some(Action::Key { combo: key }),
            CuaAction::WaitMs { .. } => None,
            CuaAction::DragPath { .. } => None,
            CuaAction::Screenshot => None,
            CuaAction::Unknown(_) => None,
        }
    }
}

#[async_trait]
impl Reasoner for CuaReasoner {
    async fn think(
        &self,
        goal: &Goal,
        _memory: &Memory,
        snapshot: &Snapshot,
        _last_error: Option<&AgentError>,
    ) -> Result<Thought, AgentError> {
        let mut st = self.state.lock().await;

        // If we are awaiting to send a screenshot for a prior computer_call
        if st.awaiting_screenshot {
            let b64 = snapshot
                .image_base64
                .clone()
                .ok_or_else(|| AgentError::Reasoner("missing snapshot image".into()))?;
            let call_id = st
                .pending_call_id
                .clone()
                .ok_or_else(|| AgentError::Reasoner("missing call_id".into()))?;
            let resp = self
                .client
                .send_computer_output(
                    &call_id,
                    CuaToolImage { r#type: "input_image".into(), mime_type: "image/png".into(), data_base64: b64 },
                    st.previous.as_ref(),
                    Some(&st.pending_safety_checks),
                )
                .await
                .map_err(|e| AgentError::Reasoner(e.to_string()))?;

            match resp {
                CuaOutput::Message { text } => {
                    st.previous = st.previous.take(); // end thread on message
                    st.pending_call_id = None;
                    st.pending_safety_checks.clear();
                    st.awaiting_screenshot = false;
                    if self.cfg.stop_on_message {
                        st.done_message = Some(text.clone());
                    }
                    return Ok(Thought { plan: text, action: None, rationale: None });
                }
                CuaOutput::ComputerCall { call_id, action, requires_screenshot, response_id, safety_checks } => {
                    st.previous = Some(response_id);
                    st.pending_call_id = Some(call_id);
                    st.pending_safety_checks = safety_checks;
                    st.awaiting_screenshot = requires_screenshot;
                    let mapped = Self::map_cua_action(action);
                    return Ok(Thought { plan: String::new(), action: mapped, rationale: None });
                }
                CuaOutput::Done { response_id } => {
                    st.previous = Some(response_id);
                    st.pending_call_id = None;
                    st.pending_safety_checks.clear();
                    st.awaiting_screenshot = false;
                    st.done_message = Some("done".into());
                    return Ok(Thought { plan: "done".into(), action: None, rationale: None });
                }
            }
        }

        // Start or continue a turn
        let composed = Self::compose_instructions(&self.instructions, goal);
        // Only append extra_user_text when not mid-thread to avoid tool-output expectation mismatches
        let extra = if st.previous.is_none() { self.cfg.auto_confirm_text.clone() } else { None };
        let input = crate::cua::TurnInput { instructions: composed, current_url: snapshot.url.clone(), extra_user_text: extra };
        let out = self
            .client
            .turn(input, st.previous.as_ref())
            .await
            .map_err(|e| AgentError::Reasoner(e.to_string()))?;

        match out {
            CuaOutput::Message { text } => {
                st.previous = st.previous.take();
                if self.cfg.stop_on_message {
                    st.done_message = Some(text.clone());
                }
                Ok(Thought { plan: text, action: None, rationale: None })
            }
            CuaOutput::ComputerCall { call_id, action, requires_screenshot, response_id, safety_checks } => {
                st.previous = Some(response_id);
                st.pending_call_id = Some(call_id);
                st.pending_safety_checks = safety_checks;
                st.awaiting_screenshot = requires_screenshot;
                let mapped = Self::map_cua_action(action);
                Ok(Thought { plan: String::new(), action: mapped, rationale: None })
            }
            CuaOutput::Done { response_id } => {
                st.previous = Some(response_id);
                st.done_message = Some("done".into());
                Ok(Thought { plan: "done".into(), action: None, rationale: None })
            }
        }
    }

    async fn success(
        &self,
        _goal: &Goal,
        _snapshot: &Snapshot,
        _memory: &Memory,
    ) -> Result<bool, AgentError> {
        let st = self.state.lock().await;
        if self.cfg.stop_on_message {
            Ok(st.done_message.is_some())
        } else {
            Ok(false)
        }
    }
}
