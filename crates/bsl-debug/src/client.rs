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

    #[error("debug server refused {cmd} (HTTP {status}): {message}")]
    Server { cmd: String, status: u16, message: String },

    #[error("evaluation failed: {0}")]
    EvalFailed(String),

    #[error("debug error: {0}")]
    Debug(#[from] DebugError),
}

impl DebugClient {
    pub fn new(host: &str, port: u16, debugger_id: &str, infobase: &str) -> Self {
        let client =
            Client::builder().user_agent("1CV8").build().expect("failed to create HTTP client");

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

    pub fn get_call_stack(&self, target_id: &str) -> Result<Vec<StackFrame>, ClientError> {
        let body = xml::build_get_callstack_request(&self.debugger_id, &self.infobase, target_id);
        let data = self.post("getCallStack", &body)?;
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
        // Deliberately tolerant, unlike `post`: a transient non-200 on the polling channel must
        // not kill the session.
        let status = resp.status();
        let data = resp.bytes()?.to_vec();
        debug!(status = %status, len = data.len(), "ping response");
        if !data.is_empty() {
            debug!(body = %String::from_utf8_lossy(&data), "ping body");
        }
        Ok(events::parse_ping_events(&data))
    }

    fn post(&self, cmd: &str, body: &[u8]) -> Result<Vec<u8>, ClientError> {
        let url = format!("{}/rdbg?cmd={cmd}", self.base_url);
        debug!(cmd, body = %String::from_utf8_lossy(body), "debug server request");

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/xml")
            .body(body.to_vec())
            .send()?;

        let status = resp.status();
        let data = resp.bytes()?.to_vec();
        debug!(cmd, status = %status, response = %String::from_utf8_lossy(&data),
            "debug server response");

        if !status.is_success() {
            let message = responses::exception_descr(&data).unwrap_or_else(|| {
                String::from_utf8_lossy(&data).chars().take(300).collect::<String>().trim().into()
            });
            return Err(ClientError::Server {
                cmd: cmd.to_string(),
                status: status.as_u16(),
                message,
            });
        }

        Ok(data)
    }
}
