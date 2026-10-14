use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, info};
use uuid::Uuid;

use crate::client::{ClientError, DebugClient};
use crate::listener::EventListener;
use crate::module_index::ModuleIndex;
use crate::types::base::{CalcPathItem, ModuleId, StepAction, ViewInterface};
use crate::types::events::DebugEvent;
use crate::types::responses::{self, EvalPayload};
use crate::types::xml::BreakpointDef;

pub struct DebugSession {
    client: Arc<DebugClient>,
    _listener: EventListener,
    events: mpsc::UnboundedReceiver<DebugEvent>,
    index: ModuleIndex,
    attached_targets: HashMap<String, String>,
    auto_attach: Vec<String>,
    breakpoints: Vec<BreakpointDef>,
    /// The target commands act on.
    stopped_target: Option<String>,
    /// Client and server can be stopped at once, so one slot cannot tell whether the target a
    /// command is about to reach is still halted.
    stopped_targets: std::collections::HashSet<String>,
    last_stop: Option<StopEvent>,
    watches: Vec<String>,
    pending_eval_results: HashMap<String, Vec<u8>>,
    /// Listed once while connecting, because a second `getDbgTargets` can answer differently and
    /// its failure must not undo an attach that already happened.
    known_targets: Vec<TargetInfo>,
    setup_warnings: Vec<String>,
}

pub struct DebugConfig {
    pub host: String,
    pub port: u16,
    pub infobase: String,
    pub config_root: PathBuf,
    pub extensions: Vec<(String, PathBuf)>,
    pub auto_attach: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StopEvent {
    pub reason: StopReason,
    pub target_id: String,
    pub module: String,
    pub line: u32,
    pub stack: Vec<FrameInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum StopReason {
    Breakpoint,
    Step,
    Exception { message: String },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FrameInfo {
    pub module_path: String,
    pub module_name: String,
    pub line: u32,
    pub presentation: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Variable {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub expandable: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EvalResult {
    pub value: String,
    pub type_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TargetInfo {
    pub id: String,
    pub target_type: String,
    pub user_name: String,
    pub infobase_alias: String,
    pub state: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WatchResult {
    pub expression: String,
    pub value: Option<EvalResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceLine {
    pub line: u32,
    pub text: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StopSnapshot {
    pub reason: StopReason,
    pub target_id: String,
    pub module: String,
    pub line: u32,
    pub source_context: Vec<SourceLine>,
    pub stack: Vec<FrameInfo>,
    pub locals: Vec<Variable>,
    pub watches: Vec<WatchResult>,
}

impl DebugSession {
    pub fn connect(config: DebugConfig) -> Result<Self, ClientError> {
        let debugger_id = Uuid::new_v4().to_string();

        let ext_refs: Vec<(&str, &Path)> =
            config.extensions.iter().map(|(n, p)| (n.as_str(), p.as_path())).collect();
        let index =
            ModuleIndex::scan(&config.config_root, &ext_refs).map_err(ClientError::Debug)?;
        info!(modules = index.len(), "module index built");

        let client =
            Arc::new(DebugClient::new(&config.host, config.port, &debugger_id, &config.infobase));

        let result = client.attach()?;
        match &result {
            crate::types::responses::AttachResult::Ok => {
                info!("attached to debug server");
            }
            crate::types::responses::AttachResult::IBInDebug => {
                info!("infobase already in debug mode — proceeding");
            }
            other => {
                return Err(ClientError::AttachFailed(
                    other.error_message().unwrap_or("unknown error").to_string(),
                ));
            }
        }

        // Both are conveniences the session works without, and the HTTP status of neither used to
        // be read at all: a server that refuses one must not now cost the caller the whole attach.
        let mut setup_warnings: Vec<String> = Vec::new();
        if let Err(e) = client.init_settings() {
            setup_warnings.push(format!("initSettings refused: {e}"));
        }

        if !config.auto_attach.is_empty() {
            let types: Vec<&str> = config.auto_attach.iter().map(|s| s.as_str()).collect();
            if let Err(e) = client.set_auto_attach(&types) {
                setup_warnings.push(format!("setAutoAttachSettings refused: {e}"));
            }
        }

        let (listener, events) = EventListener::start(client.clone(), 50);

        let mut session = Self {
            client,
            _listener: listener,
            events,
            index,
            attached_targets: HashMap::new(),
            auto_attach: config.auto_attach,
            breakpoints: Vec::new(),
            stopped_target: None,
            stopped_targets: std::collections::HashSet::new(),
            last_stop: None,
            watches: Vec::new(),
            pending_eval_results: HashMap::new(),
            known_targets: Vec::new(),
            setup_warnings,
        };

        // `setAutoAttachSettings` only covers targets that start later, so a client that was
        // already running would never be attached and no breakpoint would ever be hit.
        session.attach_running_targets();

        Ok(session)
    }

    fn attach_running_targets(&mut self) {
        let targets = match self.client.get_targets() {
            Ok(targets) => targets,
            Err(e) => {
                tracing::warn!(error = %e, "could not list running debug targets");
                self.setup_warnings.push(format!("getDbgTargets refused: {e}"));
                return;
            }
        };

        self.known_targets = targets
            .iter()
            .map(|t| TargetInfo {
                id: t.id.clone(),
                target_type: t.target_type.clone(),
                user_name: t.user_name.clone(),
                infobase_alias: t.infobase_alias.clone(),
                state: t.state.clone(),
            })
            .collect();

        for target in targets {
            if !self.auto_attach_allows(&target.target_type) {
                debug!(
                    target_id = %target.id,
                    target_type = %target.target_type,
                    "running target skipped by auto_attach"
                );
                continue;
            }
            match self.client.attach_targets(&[&target.id]) {
                Ok(()) => {
                    info!(target_id = %target.id, target_type = %target.target_type,
                        "attached to a running target");
                    self.attached_targets.insert(target.id, target.target_type);
                }
                Err(e) => tracing::warn!(target_id = %target.id, error = %e,
                    "could not attach a running debug target"),
            }
        }
    }

    /// An empty `auto_attach` attaches everything; target types are compared case-insensitively
    /// because they come back from the server, not from our own table.
    fn auto_attach_allows(&self, target_type: &str) -> bool {
        self.auto_attach.is_empty()
            || self.auto_attach.iter().any(|t| t.eq_ignore_ascii_case(target_type))
    }

    pub fn attached_count(&self) -> usize {
        self.attached_targets.len()
    }

    pub fn known_targets(&self) -> &[TargetInfo] {
        &self.known_targets
    }

    /// Non-fatal refusals collected during the handshake, so `attach` can show them instead of
    /// leaving the caller with a session that is quietly less capable than it looks.
    pub fn setup_warnings(&self) -> &[String] {
        &self.setup_warnings
    }

    pub fn disconnect(self) -> Result<(), ClientError> {
        self._listener.stop();
        // Whether a target left attached is why the thin client then refuses to close on
        // WM_CLOSE is unestablished; this only makes the teardown symmetric.
        for id in self.attached_targets.keys() {
            if let Err(e) = self.client.detach_targets(&[id.as_str()]) {
                debug!(target_id = %id, error = %e, "detaching a target on disconnect failed");
            }
        }
        self.client.detach()?;
        info!("detached from debug server");
        Ok(())
    }

    pub fn set_breakpoint(
        &mut self,
        module_name: &str,
        line: u32,
        condition: Option<&str>,
    ) -> Result<(), ClientError> {
        let (module_id, _path) = self.index.resolve_name(module_name).ok_or_else(|| {
            ClientError::Debug(crate::error::DebugError::ModuleNotFound(module_name.to_string()))
        })?;

        self.breakpoints.push(BreakpointDef {
            extension: module_id.extension.clone(),
            object_id: module_id.object_id.clone(),
            property_id: module_id.property_id.clone(),
            line,
            condition: condition.map(|s| s.to_string()),
        });

        // A rejected list must not leave a phantom breakpoint behind: every later
        // `setBreakpoints` sends the whole list and would be rejected for the same reason.
        if let Err(e) = self.client.set_breakpoints(&self.breakpoints) {
            self.breakpoints.pop();
            return Err(e);
        }
        info!(module_name, line, "breakpoint set");
        Ok(())
    }

    pub fn set_breakpoint_by_path(
        &mut self,
        path: &Path,
        line: u32,
        condition: Option<&str>,
    ) -> Result<(), ClientError> {
        let module_id = self.index.module_by_path(path).ok_or_else(|| {
            ClientError::Debug(crate::error::DebugError::ModuleNotFound(path.display().to_string()))
        })?;

        self.breakpoints.push(BreakpointDef {
            extension: module_id.extension.clone(),
            object_id: module_id.object_id.clone(),
            property_id: module_id.property_id.clone(),
            line,
            condition: condition.map(|s| s.to_string()),
        });

        if let Err(e) = self.client.set_breakpoints(&self.breakpoints) {
            self.breakpoints.pop();
            return Err(e);
        }
        Ok(())
    }

    pub fn remove_breakpoint(&mut self, module_name: &str, line: u32) -> Result<bool, ClientError> {
        let (module_id, _path) = match self.index.resolve_name(module_name) {
            Some(v) => v,
            None => return Ok(false),
        };
        let before = self.breakpoints.clone();
        self.breakpoints.retain(|bp| {
            !(bp.object_id == module_id.object_id
                && bp.property_id == module_id.property_id
                && bp.extension == module_id.extension
                && bp.line == line)
        });
        if self.breakpoints.len() != before.len() {
            if let Err(e) = self.client.set_breakpoints(&self.breakpoints) {
                self.breakpoints = before;
                return Err(e);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn remove_breakpoint_by_path(
        &mut self,
        path: &Path,
        line: u32,
    ) -> Result<bool, ClientError> {
        let module_id = match self.index.module_by_path(path) {
            Some(v) => v,
            None => return Ok(false),
        };
        let before = self.breakpoints.clone();
        self.breakpoints.retain(|bp| {
            !(bp.object_id == module_id.object_id
                && bp.property_id == module_id.property_id
                && bp.extension == module_id.extension
                && bp.line == line)
        });
        if self.breakpoints.len() != before.len() {
            if let Err(e) = self.client.set_breakpoints(&self.breakpoints) {
                self.breakpoints = before;
                return Err(e);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn clear_breakpoints(&mut self) -> Result<(), ClientError> {
        let before = std::mem::take(&mut self.breakpoints);
        if let Err(e) = self.client.set_breakpoints(&self.breakpoints) {
            self.breakpoints = before;
            return Err(e);
        }
        Ok(())
    }

    pub fn break_on_error(&self, filter: Option<&str>) -> Result<(), ClientError> {
        self.client.set_break_on_error(true, filter)
    }

    pub fn ignore_errors(&self) -> Result<(), ClientError> {
        self.client.set_break_on_error(false, None)
    }

    pub fn module_count(&self) -> usize {
        self.index.len()
    }

    pub fn module_names(&self) -> Vec<&str> {
        self.index.all_names()
    }

    pub fn targets(&self) -> Result<Vec<TargetInfo>, ClientError> {
        let targets = self.client.get_targets()?;
        Ok(targets
            .into_iter()
            .map(|t| TargetInfo {
                id: t.id,
                target_type: t.target_type,
                user_name: t.user_name,
                infobase_alias: t.infobase_alias,
                state: t.state,
            })
            .collect())
    }

    pub fn stopped_target(&self) -> Option<&str> {
        self.stopped_target.as_deref()
    }

    pub fn stopped_count(&self) -> usize {
        self.stopped_targets.len()
    }

    pub fn target_type_of(&self, id: &str) -> Option<&str> {
        self.attached_targets.get(id).map(|t| t.as_str())
    }

    pub fn last_stop(&self) -> Option<&StopEvent> {
        self.last_stop.as_ref()
    }

    pub fn wait_for_stop(&mut self, timeout: Duration) -> Result<Option<StopEvent>, ClientError> {
        // The freshest queued stop wins: an older one describes a position the caller has
        // already resumed past, and commands would then act somewhere else entirely.
        let mut queued: Option<StopEvent> = None;
        while let Ok(event) = self.events.try_recv() {
            if let Some(stop) = self.handle_event(event)? {
                queued = Some(stop);
            }
        }
        if let Some(stop) = queued {
            self.last_stop = Some(stop.clone());
            return Ok(Some(stop));
        }

        let deadline = std::time::Instant::now() + timeout;

        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    if let Some(stop) = self.handle_event(event)? {
                        self.last_stop = Some(stop.clone());
                        return Ok(Some(stop));
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return Ok(None);
                }
            }
        }
    }

    pub fn step(&mut self, action: StepAction) -> Result<(), ClientError> {
        let target_id = self.require_stopped_target()?.to_string();

        let action_str = match action {
            StepAction::Next => "Step",
            StepAction::StepIn => "StepIn",
            StepAction::StepOut => "StepOut",
            StepAction::Continue => "Continue",
        };

        self.client.step(&target_id, action_str)?;

        self.stopped_targets.remove(&target_id);
        if self.stopped_target.as_deref() == Some(target_id.as_str()) {
            self.stopped_target = self.stopped_targets.iter().next().cloned();
        }
        Ok(())
    }

    pub fn continue_execution(&mut self) -> Result<(), ClientError> {
        self.step(StepAction::Continue)
    }

    /// The target commands act on, and only while it is still halted: the debug server answers
    /// HTTP 400 for an evaluation against a running target, and a stale id sends it there.
    fn require_stopped_target(&self) -> Result<&str, ClientError> {
        if let Some(ref id) = self.stopped_target {
            if self.stopped_targets.contains(id) {
                return Ok(id);
            }
        }
        Err(ClientError::NotStopped)
    }

    pub fn call_stack(&self) -> Result<Vec<FrameInfo>, ClientError> {
        let target_id = self.require_stopped_target()?;

        let frames = self.client.get_call_stack(target_id)?;

        Ok(frames
            .into_iter()
            .map(|f| {
                let module_id = ModuleId {
                    extension: f.module_extension,
                    object_id: f.module_object_id,
                    property_id: f.module_property_id,
                };
                let module_path = self
                    .index
                    .path_by_module(&module_id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();

                FrameInfo {
                    module_path,
                    module_name: module_id.to_string(),
                    line: f.line_no,
                    presentation: f.presentation,
                }
            })
            .collect())
    }

    pub fn locals(&mut self, stack_level: u32) -> Result<Vec<Variable>, ClientError> {
        let target_id = self.require_stopped_target()?.to_string();

        let result_id = Uuid::new_v4().to_string();
        let payload = self.client.eval_local_vars(&target_id, stack_level as i64, &result_id)?;
        let payload = self.settle_eval(payload, &result_id);

        if let Some(message) = payload.error {
            return Err(ClientError::EvalFailed(message));
        }

        Ok(payload
            .values
            .into_iter()
            .map(|v| Variable {
                name: v.name,
                type_name: v.type_name,
                value: v.presentation,
                expandable: v.is_expandable,
            })
            .collect())
    }

    /// An evaluation answers HTTP 200 with an empty body and reports on the ping channel, so an
    /// empty synchronous payload is a promise of a later one, not a result.
    fn settle_eval(&mut self, payload: EvalPayload, result_id: &str) -> EvalPayload {
        if payload.values.is_empty() && payload.error.is_none() {
            self.wait_for_eval_result(result_id, Duration::from_secs(5))
        } else {
            payload
        }
    }

    fn wait_for_eval_result(&mut self, result_id: &str, timeout: Duration) -> EvalPayload {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            if let Some(xml) = self.pending_eval_results.remove(result_id) {
                return responses::parse_eval_result(&xml);
            }

            match self.events.try_recv() {
                Ok(event) => {
                    let _ = self.handle_event(event);
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        debug!(result_id, "eval result timeout");
                        return EvalPayload::default();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return EvalPayload::default();
                }
            }
        }
    }

    pub fn eval(&mut self, expression: &str, stack_level: u32) -> Result<EvalResult, ClientError> {
        let target_id = self.require_stopped_target()?.to_string();

        let result_id = Uuid::new_v4().to_string();
        let payload =
            self.client.eval_expr(&target_id, expression, stack_level as i64, &result_id)?;
        let payload = self.settle_eval(payload, &result_id);

        if let Some(message) = payload.error {
            return Err(ClientError::EvalFailed(message));
        }

        match payload.values.into_iter().next() {
            Some(first) => Ok(EvalResult { value: first.presentation, type_name: first.type_name }),
            // An empty value here is not an empty variable: nothing came back at all, and
            // returning one would read as a successful evaluation of nothing.
            None => Err(ClientError::EvalFailed(
                "the debug server returned no result within 5 s".into(),
            )),
        }
    }

    pub fn expand(
        &mut self,
        path: &[CalcPathItem],
        view: ViewInterface,
        stack_level: u32,
    ) -> Result<Vec<Variable>, ClientError> {
        let target_id = self.require_stopped_target()?.to_string();

        let result_id = Uuid::new_v4().to_string();
        let payload =
            self.client.eval_expand(&target_id, path, view, stack_level as i64, &result_id)?;
        let payload = self.settle_eval(payload, &result_id);

        if let Some(message) = payload.error {
            return Err(ClientError::EvalFailed(message));
        }

        Ok(payload
            .values
            .into_iter()
            .map(|v| Variable {
                name: v.name,
                type_name: v.type_name,
                value: v.presentation,
                expandable: v.is_expandable,
            })
            .collect())
    }

    pub fn add_watch(&mut self, expression: &str) {
        if !self.watches.contains(&expression.to_string()) {
            self.watches.push(expression.to_string());
        }
    }

    pub fn remove_watch(&mut self, expression: &str) {
        self.watches.retain(|w| w != expression);
    }

    pub fn watch_list(&self) -> &[String] {
        &self.watches
    }

    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.len()
    }

    pub fn eval_watches(&mut self, stack_level: u32) -> Vec<WatchResult> {
        let expressions: Vec<String> = self.watches.clone();
        expressions
            .iter()
            .map(|expr| match self.eval(expr, stack_level) {
                Ok(result) => {
                    WatchResult { expression: expr.clone(), value: Some(result), error: None }
                }
                Err(e) => WatchResult {
                    expression: expr.clone(),
                    value: None,
                    error: Some(e.to_string()),
                },
            })
            .collect()
    }

    pub fn source_context(
        &self,
        file_path: &str,
        current_line: u32,
        context: u32,
    ) -> Result<Vec<SourceLine>, ClientError> {
        let path = std::path::Path::new(file_path);
        let content = std::fs::read_to_string(path)
            .map_err(|e| ClientError::Debug(crate::error::DebugError::Io(e)))?;

        let start = current_line.saturating_sub(context).max(1);
        let end = current_line + context;

        Ok(content
            .lines()
            .enumerate()
            .filter_map(|(i, text)| {
                let line_num = (i + 1) as u32;
                if line_num >= start && line_num <= end {
                    Some(SourceLine {
                        line: line_num,
                        text: text.to_string(),
                        is_current: line_num == current_line,
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    pub fn snapshot_from_stop(
        &mut self,
        stop: &StopEvent,
        source_context_lines: u32,
    ) -> Result<StopSnapshot, ClientError> {
        let locals = self.locals(0).unwrap_or_default();
        let watches = self.eval_watches(0);

        let source_context = if !stop.module.is_empty() && stop.line > 0 {
            self.source_context(&stop.module, stop.line, source_context_lines).unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(StopSnapshot {
            reason: stop.reason.clone(),
            target_id: stop.target_id.clone(),
            module: stop.module.clone(),
            line: stop.line,
            source_context,
            stack: stop.stack.clone(),
            locals,
            watches,
        })
    }

    pub fn module_index(&self) -> &ModuleIndex {
        &self.index
    }

    fn breakpoint_at(&self, module_id: &ModuleId, line: u32) -> bool {
        self.breakpoints.iter().any(|bp| {
            bp.extension == module_id.extension
                && bp.object_id == module_id.object_id
                && bp.property_id == module_id.property_id
                && bp.line == line
        })
    }

    fn handle_event(&mut self, event: DebugEvent) -> Result<Option<StopEvent>, ClientError> {
        match event {
            DebugEvent::TargetStarted { target_id, target_type } => {
                if !self.auto_attach_allows(&target_type) {
                    debug!(target_id, target_type, "target skipped by auto_attach");
                    return Ok(None);
                }
                info!(target_id, target_type, "target started");
                self.client.attach_targets(&[&target_id])?;
                self.attached_targets.insert(target_id.clone(), target_type);
                Ok(None)
            }
            DebugEvent::TargetQuit { target_id } => {
                info!(target_id, "target quit");
                self.attached_targets.remove(&target_id);
                self.stopped_targets.remove(&target_id);
                if self.stopped_target.as_deref() == Some(&target_id) {
                    self.stopped_target = self.stopped_targets.iter().next().cloned();
                }
                Ok(None)
            }
            DebugEvent::CallStackFormed {
                target_id,
                line_no,
                module_extension,
                module_object_id,
                module_property_id,
                send_message_only,
                call_stack,
                ..
            } => {
                if send_message_only {
                    self.client.step(&target_id, "Continue")?;
                    return Ok(None);
                }

                self.stopped_targets.insert(target_id.clone());
                self.stopped_target = Some(target_id.clone());

                let module_id = ModuleId {
                    extension: module_extension,
                    object_id: module_object_id,
                    property_id: module_property_id,
                };
                let module_path = self
                    .index
                    .path_by_module(&module_id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| module_id.to_string());

                // Neither 8.3.17 nor 8.3.27 puts a moduleID or a lineNo at the top level of this
                // event: both sit inside the innermost `callStack` frame, which is therefore the
                // only description of where execution stopped.
                let innermost = call_stack.first().map(|f| {
                    (
                        ModuleId {
                            extension: f.extension.clone(),
                            object_id: f.object_id.clone(),
                            property_id: f.property_id.clone(),
                        },
                        f.line_no,
                    )
                });

                // `stopByBP` stays true after a step too, so only the position can tell the two
                // apart: a stop is a breakpoint stop exactly where this session put one. A
                // breakpoint removed between the stop and this classification reads as a step.
                let reason_at = if line_no > 0 && !module_id.object_id.is_empty() {
                    Some((module_id.clone(), line_no))
                } else {
                    innermost
                };
                let reason = match &reason_at {
                    Some((mid, line)) if self.breakpoint_at(mid, *line) => StopReason::Breakpoint,
                    _ => StopReason::Step,
                };

                let stack: Vec<FrameInfo> = call_stack
                    .into_iter()
                    .map(|f| {
                        let mid = ModuleId {
                            extension: f.extension,
                            object_id: f.object_id,
                            property_id: f.property_id,
                        };
                        let path = self
                            .index
                            .path_by_module(&mid)
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        FrameInfo {
                            module_path: path,
                            module_name: mid.to_string(),
                            line: f.line_no,
                            presentation: f.presentation,
                        }
                    })
                    .collect();

                // Index 0 is the current frame, so the innermost position is the first one.
                let (stop_module, stop_line) = if line_no > 0 && !module_path.is_empty() {
                    (module_path, line_no)
                } else if let Some(first) = stack.first() {
                    (first.module_path.clone(), first.line)
                } else {
                    (module_path, line_no)
                };

                Ok(Some(StopEvent {
                    reason,
                    target_id,
                    module: stop_module,
                    line: stop_line,
                    stack,
                }))
            }
            DebugEvent::RuntimeException {
                target_id,
                description,
                line_no,
                module_extension,
                module_object_id,
                module_property_id,
            } => {
                self.stopped_targets.insert(target_id.clone());
                self.stopped_target = Some(target_id.clone());

                let module_id = ModuleId {
                    extension: module_extension,
                    object_id: module_object_id,
                    property_id: module_property_id,
                };
                let module_path = self
                    .index
                    .path_by_module(&module_id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| module_id.to_string());

                let stack = self.call_stack().unwrap_or_default();

                Ok(Some(StopEvent {
                    reason: StopReason::Exception { message: description },
                    target_id,
                    module: module_path,
                    line: line_no,
                    stack,
                }))
            }
            DebugEvent::ExprEvaluated { result_id, raw_xml } => {
                debug!(result_id, "async expression evaluated, buffering");
                self.pending_eval_results.insert(result_id, raw_xml);
                Ok(None)
            }
        }
    }
}
