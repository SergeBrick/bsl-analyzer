use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::blocking::Client;
use tracing::{debug, warn};

use crate::error::DebugError;
use crate::types::events::{self, DebugEvent};
use crate::types::responses::{self, AttachResult, DebugTarget, EvalPayload, StackFrame};
use crate::types::xml;

pub struct DebugClient {
    client: Client,
    base_url: String,
    pub debugger_id: String,
    pub infobase: String,
    /// Negotiated on the first rejected `setBreakpoints`: 8.3.27 needs `breakOnCondition`,
    /// 8.3.17 refuses the request over it, and nothing in the protocol announces which.
    break_on_condition: AtomicBool,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("attach failed: {0}")]
    AttachFailed(String),

    #[error("no target is stopped — call wait_stop first")]
    NotStopped,

    #[error("target {target} is not stopped; stopped targets: {stopped}")]
    TargetNotStopped { target: String, stopped: String },

    #[error("debug server refused {cmd} (HTTP {status}): {message}")]
    Server { cmd: String, status: u16, message: String },

    #[error("the debug server did not answer {cmd} within {secs} s")]
    Timeout { cmd: String, secs: u64 },

    #[error("evaluation failed: {0}")]
    EvalFailed(String),

    #[error("debug error: {0}")]
    Debug(#[from] DebugError),
}

impl DebugClient {
    pub fn new(host: &str, port: u16, debugger_id: &str, infobase: &str) -> Self {
        let client = Client::builder()
            .user_agent("1CV8")
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to create HTTP client");

        Self {
            client,
            base_url: format!("http://{host}:{port}/e1crdbg"),
            debugger_id: debugger_id.to_string(),
            infobase: infobase.to_string(),
            break_on_condition: AtomicBool::new(true),
        }
    }

    pub fn attach(&self) -> Result<AttachResult, ClientError> {
        let body = xml::build_attach_request(&self.debugger_id, &self.infobase);
        let data = self.post("attachDebugUI", &body)?;
        Ok(AttachResult::parse(&data))
    }

    pub fn detach(&self) -> Result<(), ClientError> {
        let body = xml::build_detach_request(&self.debugger_id, &self.infobase);
        self.post("detachDebugUI", &body)?;
        Ok(())
    }

    pub fn init_settings(&self) -> Result<(), ClientError> {
        let body = xml::build_init_settings_request(&self.debugger_id, &self.infobase);
        self.post("initSettings", &body)?;
        Ok(())
    }

    pub fn set_auto_attach(&self, target_types: &[&str]) -> Result<(), ClientError> {
        let body =
            xml::build_set_auto_attach_request(&self.debugger_id, &self.infobase, target_types);
        self.post("setAutoAttachSettings", &body)?;
        Ok(())
    }

    /// Run state of every target of the infobase. `getDbgTargets` carries no state at all, and the
    /// stop event is never replayed to a debugger that registers later, so this is the only way to
    /// learn that a target is already halted.
    pub fn get_all_target_states(&self) -> Result<Vec<DebugTarget>, ClientError> {
        let body = xml::build_get_all_target_states_request(&self.debugger_id, &self.infobase);
        let data = self.post("getDbgAllTargetStates", &body)?;
        Ok(responses::parse_targets(&data))
    }

    pub fn get_targets(&self) -> Result<Vec<DebugTarget>, ClientError> {
        let body = xml::build_get_targets_request(&self.debugger_id, &self.infobase);
        let data = self.post("getDbgTargets", &body)?;
        Ok(responses::parse_targets(&data))
    }

    pub fn attach_targets(&self, target_ids: &[&str]) -> Result<(), ClientError> {
        let body =
            xml::build_attach_targets_request(&self.debugger_id, &self.infobase, true, target_ids);
        self.post("attachDetachDbgTargets", &body)?;
        Ok(())
    }

    pub fn detach_targets(&self, target_ids: &[&str]) -> Result<(), ClientError> {
        let body =
            xml::build_attach_targets_request(&self.debugger_id, &self.infobase, false, target_ids);
        self.post("attachDetachDbgTargets", &body)?;
        Ok(())
    }

    pub fn set_breakpoints(&self, breakpoints: &[xml::BreakpointDef]) -> Result<(), ClientError> {
        let flag = self.break_on_condition.load(Ordering::Relaxed);
        let body = xml::build_set_breakpoints_request(
            &self.debugger_id,
            &self.infobase,
            breakpoints,
            flag,
        );
        let err = match self.post("setBreakpoints", &body) {
            Ok(_) => return Ok(()),
            Err(err) => err,
        };

        // The rejection takes down the whole list, not just the conditional breakpoint, so the
        // one retry that re-shapes the request is what keeps the rest of the session alive.
        let over_break_on_condition = matches!(
            &err,
            ClientError::Server { message, .. } if message.contains("breakOnCondition")
        );
        if !flag || !over_break_on_condition {
            return Err(err);
        }

        self.break_on_condition.store(false, Ordering::Relaxed);
        warn!(
            "debug server does not accept breakOnCondition; conditional breakpoints now go \
             without it (8.3.17-shaped server)"
        );
        let body = xml::build_set_breakpoints_request(
            &self.debugger_id,
            &self.infobase,
            breakpoints,
            false,
        );
        self.post("setBreakpoints", &body)?;
        Ok(())
    }

    pub fn set_break_on_error(&self, stop: bool, filter: Option<&str>) -> Result<(), ClientError> {
        let body =
            xml::build_set_break_on_rte_request(&self.debugger_id, &self.infobase, stop, filter);
        self.post("setBreakOnRTE", &body)?;
        Ok(())
    }

    pub fn step(&self, target_id: &str, action: &str) -> Result<(), ClientError> {
        let body = xml::build_step_request(&self.debugger_id, &self.infobase, target_id, action);
        self.post("step", &body)?;
        Ok(())
    }

    /// The server answers only once the halted target has reported its stack, and only while
    /// this debugger keeps polling its event channel. Measured live (03.10.2026, both versions):
    /// from 1.6 s to about 10 s, at times no answer within 30 s, and at times the answer after
    /// the wait is an empty HTTP 204; a repeated request for the same halt answers 204 at once. Hence the bound,
    /// and an empty result is an answer, not an error.
    pub fn get_call_stack(
        &self,
        target_id: &str,
        wait: std::time::Duration,
    ) -> Result<Vec<StackFrame>, ClientError> {
        let body = xml::build_get_callstack_request(&self.debugger_id, &self.infobase, target_id);
        let data = self.post_bounded("getCallStack", &body, Some(wait))?;
        Ok(responses::parse_call_stack(&data))
    }

    pub fn eval_expr(
        &self,
        target_id: &str,
        expression: &str,
        stack_level: i64,
        result_id: &str,
    ) -> Result<EvalPayload, ClientError> {
        let body = xml::build_eval_expr_request(
            &self.debugger_id,
            &self.infobase,
            target_id,
            expression,
            stack_level,
            result_id,
        );
        let data = self.post("evalExpr", &body)?;
        Ok(responses::parse_eval_result(&data))
    }

    pub fn eval_local_vars(
        &self,
        target_id: &str,
        stack_level: i64,
        result_id: &str,
    ) -> Result<EvalPayload, ClientError> {
        let body = xml::build_eval_local_vars_request(
            &self.debugger_id,
            &self.infobase,
            target_id,
            stack_level,
            result_id,
        );
        let data = self.post("evalLocalVariables", &body)?;
        Ok(responses::parse_eval_result(&data))
    }

    pub fn eval_expand(
        &self,
        target_id: &str,
        path: &[crate::types::base::CalcPathItem],
        view: crate::types::base::ViewInterface,
        stack_level: i64,
        result_id: &str,
    ) -> Result<EvalPayload, ClientError> {
        let body = xml::build_eval_expand_request(
            &self.debugger_id,
            &self.infobase,
            target_id,
            path,
            view,
            stack_level,
            result_id,
        );
        let data = self.post("evalExpr", &body)?;
        Ok(responses::parse_eval_result(&data))
    }

    pub fn ping(&self) -> Result<Vec<DebugEvent>, ClientError> {
        let url =
            format!("{}/rdbg?cmd=pingDebugUIParams&dbgui={}", self.base_url, self.debugger_id);
        let resp = self.client.post(&url).send()?;
        let status = resp.status();
        let data = resp.bytes()?.to_vec();
        debug!(status = %status, len = data.len(), "ping response");
        if !data.is_empty() {
            debug!(body = %String::from_utf8_lossy(&data), "ping body");
        }
        // A refusal here is how the server says this debugger is no longer registered ("UI+ -
        // часть отладки не зарегистрирована"): no event will ever arrive, so it must reach the
        // caller instead of reading as a quiet channel.
        if !status.is_success() {
            return Err(server_error("pingDebugUIParams", status.as_u16(), &data));
        }
        Ok(events::parse_ping_events(&data))
    }

    fn post(&self, cmd: &str, body: &[u8]) -> Result<Vec<u8>, ClientError> {
        self.post_bounded(cmd, body, None)
    }

    fn post_bounded(
        &self,
        cmd: &str,
        body: &[u8],
        timeout: Option<std::time::Duration>,
    ) -> Result<Vec<u8>, ClientError> {
        let url = format!("{}/rdbg?cmd={cmd}", self.base_url);
        debug!(cmd, body = %String::from_utf8_lossy(body), "debug server request");

        let mut request =
            self.client.post(&url).header("Content-Type", "application/xml").body(body.to_vec());
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let resp = match request.send() {
            Ok(resp) => resp,
            Err(e) if e.is_timeout() => {
                return Err(ClientError::Timeout {
                    cmd: cmd.to_string(),
                    secs: timeout.unwrap_or(REQUEST_TIMEOUT).as_secs(),
                })
            }
            Err(e) => return Err(e.into()),
        };

        let status = resp.status();
        let data = resp.bytes()?.to_vec();
        debug!(cmd, status = %status, response = %String::from_utf8_lossy(&data),
            "debug server response");

        if !status.is_success() {
            return Err(server_error(cmd, status.as_u16(), &data));
        }

        Ok(data)
    }
}

pub const CALL_STACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Set on the HTTP client rather than left to its default, so that a timeout message can name
/// the wait it reports.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn server_error(cmd: &str, status: u16, data: &[u8]) -> ClientError {
    let message = responses::exception_descr(data).unwrap_or_else(|| {
        String::from_utf8_lossy(data).chars().take(300).collect::<String>().trim().into()
    });
    ClientError::Server { cmd: cmd.to_string(), status, message }
}
