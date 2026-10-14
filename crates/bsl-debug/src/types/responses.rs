use quick_xml::events::Event;
use quick_xml::Reader;

use super::xml::local_name_owned as local_name;

#[derive(Debug)]
pub enum AttachResult {
    Ok,
    IBNotRegistered,
    IBInDebug,
    CredentialsRequired,
    Unknown(String),
}

impl AttachResult {
    pub fn parse(data: &[u8]) -> Self {
        match read_element_text(data, "result") {
            Some(s) => match s.as_str() {
                "registered" => Self::Ok,
                "notRegistered" | "ibNotRegistered" => Self::IBNotRegistered,
                "ibInDebug" => Self::IBInDebug,
                "credentialsRequired" | "fullCredentialsRequired" => Self::CredentialsRequired,
                other => Self::Unknown(other.to_string()),
            },
            None => Self::Unknown("no result element".to_string()),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Ok => None,
            Self::IBNotRegistered => Some("infobase not registered for debugging on the server"),
            Self::IBInDebug => Some("infobase is already being debugged by another debugger"),
            Self::CredentialsRequired => Some("debug server requires authentication"),
            Self::Unknown(s) => Some(s),
        }
    }
}

/// One-line text of an `<exception>` the debug server answers a rejected request with.
///
/// Two shapes carry it — `<descr>` and `<d1p1:descr>` — and the platform puts newlines and
/// tabs inside it, so every such run becomes `"; "` and the result fits on one log line.
pub fn exception_descr(data: &[u8]) -> Option<String> {
    let raw = read_element_text(data, "descr")?;
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.trim();
    while let Some(start) = rest.find(char::is_whitespace) {
        let (head, tail) = rest.split_at(start);
        out.push_str(head);
        let run_len = tail.find(|c: char| !c.is_whitespace()).unwrap_or(tail.len());
        let (run, tail) = tail.split_at(run_len);
        if run.contains('\n') || run.contains('\t') {
            out.push_str("; ");
        } else {
            out.push_str(run);
        }
        rest = tail;
    }
    out.push_str(rest);
    Some(out.trim().to_string())
}

#[derive(Debug, Clone, Default)]
pub struct DebugTarget {
    pub id: String,
    pub seq_no: String,
    pub target_type: String,
    pub user_name: String,
    pub infobase_alias: String,
    pub state: String,
}

/// Whether a `DbgTargetState` from `getDbgAllTargetStates` means the target is halted and waits
/// for a debugger command.
///
/// A target halted on a breakpoint reports `StopOnNextLine` (state number 32) on both 8.3.17 and
/// 8.3.27, a running one `Worked` (16); there is no state literally called "stopped". `Evaluating`
/// is a halted target busy with an expression and resumes the same way.
pub fn is_suspended_state(state: &str) -> bool {
    matches!(state, "StopOnNextLine" | "Evaluating")
}

/// Targets come back under a per-command wrapper — `<…:id>` from `getDbgTargets`, `<…:item>`
/// from `step` — and the fields sit one level deeper in the second shape, inside `<targetID>`.
/// Depth carries both: depth 2 starts a target whatever it is called, and only leaves at depth
/// 3 or more are collected, so the wrapper named `id` cannot be mistaken for the inner `<id>`.
pub fn parse_targets(data: &[u8]) -> Vec<DebugTarget> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);

    let mut targets = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<DebugTarget> = None;
    let mut current_element = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                if depth == 2 {
                    current = Some(DebugTarget::default());
                } else if depth >= 3 {
                    current_element = local_name(e.name().as_ref());
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Some(target) = current.as_mut() {
                    let text = String::from_utf8_lossy(e.as_ref()).to_string();
                    match current_element.as_slice() {
                        b"id" => target.id = text,
                        b"seqNo" => target.seq_no = text,
                        // Neither recorded shape sends seqNo; seanceNo is the only sequence
                        // number there is, so it fills the field without shadowing a real seqNo.
                        b"seanceNo" => {
                            if target.seq_no.is_empty() {
                                target.seq_no = text;
                            }
                        }
                        b"targetType" => target.target_type = text,
                        b"infoBaseAlias" => target.infobase_alias = text,
                        b"state" => target.state = text,
                        b"userName" => {
                            target.user_name = decode_base64_utf8(&text).unwrap_or(text);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(_)) => {
                if depth == 2 {
                    if let Some(target) = current.take() {
                        if !target.id.is_empty() {
                            targets.push(target);
                        }
                    }
                }
                depth = depth.saturating_sub(1);
                current_element.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    targets
}

#[derive(Debug, Clone)]
pub struct StackFrame {
    pub line_no: u32,
    pub presentation: String,
    pub module_extension: String,
    pub module_object_id: String,
    pub module_property_id: String,
}

pub fn parse_call_stack(data: &[u8]) -> Vec<StackFrame> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);

    let mut frames = Vec::new();
    let mut in_call_stack = false;
    let mut in_module_id = false;
    let mut current = StackFrame {
        line_no: 0,
        presentation: String::new(),
        module_extension: String::new(),
        module_object_id: String::new(),
        module_property_id: String::new(),
    };
    let mut current_element = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_slice() {
                    b"callStack" => {
                        in_call_stack = true;
                        current = StackFrame {
                            line_no: 0,
                            presentation: String::new(),
                            module_extension: String::new(),
                            module_object_id: String::new(),
                            module_property_id: String::new(),
                        };
                    }
                    b"moduleID" if in_call_stack => in_module_id = true,
                    _ if in_call_stack => current_element = local.to_vec(),
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if in_call_stack => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                if in_module_id {
                    match current_element.as_slice() {
                        b"extensionName" => current.module_extension = text,
                        b"objectID" => current.module_object_id = text,
                        b"propertyID" => current.module_property_id = text,
                        _ => {}
                    }
                } else {
                    match current_element.as_slice() {
                        b"lineNo" => current.line_no = text.parse().unwrap_or(0),
                        b"presentation" => {
                            current.presentation = decode_base64_utf8(&text).unwrap_or(text);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_slice() {
                    b"callStack" => {
                        in_call_stack = false;
                        frames.push(current.clone());
                    }
                    b"moduleID" => in_module_id = false,
                    _ => {}
                }
                current_element.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    frames.reverse();
    frames
}

#[derive(Debug, Clone)]
pub struct VarValue {
    pub name: String,
    pub type_name: String,
    pub presentation: String,
    pub is_expandable: bool,
}

/// What one evaluation answered: values, or the platform's own error text.
///
/// The two are exclusive in practice but not in the protocol, and an empty `values` with no
/// `error` is a real answer (an empty frame) — which is why the error cannot be a value.
#[derive(Debug, Clone, Default)]
pub struct EvalPayload {
    pub values: Vec<VarValue>,
    pub error: Option<String>,
}

const EVAL_ERROR_WITHOUT_DESCRIPTION: &str =
    "the debug server reported an evaluation error without a description";

pub fn parse_eval_result(data: &[u8]) -> EvalPayload {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);

    let mut vars = Vec::new();
    let mut in_prop_info = false;
    let mut in_value_info = false;
    let mut current_name = String::new();
    let mut current_type = String::new();
    let mut current_pres = String::new();
    let mut current_expandable = false;
    let mut current_element = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_slice() {
                    b"valueOfContextPropInfo" => {
                        in_prop_info = true;
                        current_name.clear();
                        current_type.clear();
                        current_pres.clear();
                        current_expandable = false;
                    }
                    b"valueInfo" if in_prop_info => in_value_info = true,
                    _ => current_element = local.to_vec(),
                }
            }
            Ok(Event::Text(ref e)) if in_prop_info => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                if in_value_info {
                    match current_element.as_slice() {
                        b"typeName" => current_type = text,
                        b"pres" => {
                            current_pres = decode_base64_utf8(&text).unwrap_or(text);
                        }
                        b"isExpandable" => current_expandable = text == "true",
                        _ => {}
                    }
                } else if current_element.as_slice() == b"propName" {
                    current_name = text;
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_slice() {
                    b"valueOfContextPropInfo" => {
                        in_prop_info = false;
                        vars.push(VarValue {
                            name: current_name.clone(),
                            type_name: current_type.clone(),
                            presentation: current_pres.clone(),
                            is_expandable: current_expandable,
                        });
                    }
                    b"valueInfo" => in_value_info = false,
                    _ => {}
                }
                current_element.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    if vars.is_empty() {
        if let Some(pres) = read_element_text(data, "pres") {
            let type_name = read_element_text(data, "typeName").unwrap_or_default();
            let pres = decode_base64_utf8(&pres).unwrap_or(pres);
            vars.push(VarValue {
                name: String::new(),
                type_name,
                presentation: pres,
                is_expandable: false,
            });
        }
    }

    EvalPayload { values: vars, error: parse_eval_error(data) }
}

/// A failed evaluation answers HTTP 200 with an empty body and reports asynchronously, so this
/// text is the only description of what went wrong that ever reaches the caller.
fn parse_eval_error(data: &[u8]) -> Option<String> {
    let state = read_element_text(data, "evalResultState").unwrap_or_default();
    let occurred = read_element_text(data, "errorOccurred").as_deref() == Some("true");
    if !occurred && state != "withErrors" {
        return None;
    }
    // exceptionStr is base64 and may be wrapped across lines.
    let decoded = read_element_text(data, "exceptionStr")
        .map(|s| s.chars().filter(|c| !c.is_whitespace()).collect::<String>())
        .and_then(|s| decode_base64_utf8(&s))
        .filter(|s| !s.trim().is_empty());
    Some(decoded.unwrap_or_else(|| EVAL_ERROR_WITHOUT_DESCRIPTION.to_string()))
}

fn read_element_text(data: &[u8], target: &str) -> Option<String> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);
    let target_bytes = target.as_bytes();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if local_name(e.name().as_ref()) == target_bytes => {
                if let Ok(Event::Text(t)) = reader.read_event() {
                    return Some(String::from_utf8_lossy(t.as_ref()).to_string());
                }
                return Some(String::new());
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
    }
}

fn decode_base64_utf8(s: &str) -> Option<String> {
    use std::io::Read;
    let bytes = base64_decode(s.as_bytes())?;
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> =
            bytes[2..].as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes).collect();
        Some(String::from_utf16_lossy(&u16s))
    } else {
        let mut s = String::new();
        (&bytes[..]).read_to_string(&mut s).ok()?;
        Some(s)
    }
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn val(c: u8) -> Option<u8> {
        TABLE.iter().position(|&b| b == c).map(|p| p as u8)
    }

    let input: Vec<u8> =
        input.iter().copied().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);

    for chunk in input.chunks(4) {
        let mut buf = [0u8; 4];
        let mut len = 0;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                break;
            }
            buf[i] = val(b)?;
            len = i + 1;
        }
        if len >= 2 {
            out.push((buf[0] << 2) | (buf[1] >> 4));
        }
        if len >= 3 {
            out.push((buf[1] << 4) | (buf[2] >> 2));
        }
        if len >= 4 {
            out.push((buf[2] << 6) | buf[3]);
        }
    }

    Some(out)
}

/// Recorded debug server answers (03.10.2026, 8.3.27.2214) shared by the parser and session tests.
#[cfg(test)]
pub(crate) mod recorded {
    /// `getDbgAllTargetStates` while the server target was halted on a breakpoint by a debugger
    /// that then detached without resuming it.
    pub(crate) const ALL_TARGET_STATES_WITH_A_HALTED_TARGET: &str = concat!(
        r#"<response xmlns="http://v8.1c.ru/8.3/debugger/debugBaseData" xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
        "<debugRDBGRequestResponse:item><targetIDStr>SwqBgY</targetIDStr><targetID>",
        "<id>950b2b2e-bb8f-4a43-b9bf-1316114df630</id><seanceNo>2</seanceNo>",
        "<infoBaseAlias>DefAlias</infoBaseAlias><isServerInfoBase>undefined</isServerInfoBase>",
        "<targetType>ManagedClient</targetType></targetID><stateNum>16</stateNum>",
        "<state>Worked</state></debugRDBGRequestResponse:item>",
        "<debugRDBGRequestResponse:item><targetIDStr>SwqBgZ</targetIDStr><targetID>",
        "<id>2a887b1e-1151-46c8-ab56-e0078edee53d</id><seanceNo>2</seanceNo>",
        "<infoBaseAlias>DefAlias</infoBaseAlias><isServerInfoBase>undefined</isServerInfoBase>",
        "<targetType>ServerEmulation</targetType></targetID><stateNum>32</stateNum>",
        "<state>StopOnNextLine</state></debugRDBGRequestResponse:item></response>",
    );

    /// `getCallStack` for that halted target; the presentation is base64 wrapped across lines.
    pub(crate) const GET_CALL_STACK: &str = concat!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        r#"<response xmlns="http://v8.1c.ru/8.3/debugger/debugBaseData" xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
        "<debugRDBGRequestResponse:callStack><moduleIDStr>QEGBtT4dDFsB</moduleIDStr><moduleID>",
        "<objectID>5b0c1d3e-7a01-4c11-9a01-000000000001</objectID>",
        "<propertyID>d5963243-262e-4398-b4d7-fb16d06484f6</propertyID>",
        "<version>0f84ca11bff27b429159133d0f8b574500000000</version></moduleID><lineNo>4</lineNo>",
        "<presentation>0J7QsdGJ0LjQudCc0L7QtNGD0LvRjC7QotC10YHRgtCe0YLQu9Cw0LTQutC4LtCc\r\n",
        "0L7QtNGD0LvRjC7Qn9C+0YHRh9C40YLQsNGC0Ywo0J0gPSAp</presentation>",
        "<isFantom>false</isFantom></debugRDBGRequestResponse:callStack></response>",
    );
}

#[cfg(test)]
mod tests {
    //! Every document below is recorded traffic (27.09.2026, 8.3.17.1549 and 8.3.27.2214),
    //! not an illustration — the shapes are what the parsers have to survive.

    use super::*;

    const GET_DBG_TARGETS: &str = concat!(
        r#"<response xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
        "<debugRDBGRequestResponse:id><id>03432706-965e-4885-90d4-e179f5a08062</id>",
        "<seanceId>x</seanceId><seanceNo>2</seanceNo><infoBaseInstanceID>y</infoBaseInstanceID>",
        "<infoBaseAlias>DefAlias</infoBaseAlias><isServerInfoBase>undefined</isServerInfoBase>",
        "<configVersion>z</configVersion><targetType>ServerEmulation</targetType>",
        "</debugRDBGRequestResponse:id>",
        "<debugRDBGRequestResponse:id><id>f79f9782-45e7-4241-ba43-b638e458cade</id>",
        "<seanceNo>2</seanceNo><infoBaseAlias>DefAlias</infoBaseAlias>",
        "<targetType>ManagedClient</targetType></debugRDBGRequestResponse:id></response>",
    );

    const STEP_RESPONSE: &str = concat!(
        r#"<response xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
        "<debugRDBGRequestResponse:item><targetIDStr>SwqBgY</targetIDStr><targetID>",
        "<id>78791c83-e256-4569-be28-8379e7c77de8</id><seanceId>x</seanceId><seanceNo>2</seanceNo>",
        "<infoBaseInstanceID>y</infoBaseInstanceID><infoBaseAlias>DefAlias</infoBaseAlias>",
        "<isServerInfoBase>undefined</isServerInfoBase><configVersion>z</configVersion>",
        "<targetType>ServerEmulation</targetType></targetID><stateNum>16</stateNum>",
        "<state>Worked</state></debugRDBGRequestResponse:item>",
        "<debugRDBGRequestResponse:item><targetIDStr>Zzz</targetIDStr><targetID>",
        "<id>f79f9782-45e7-4241-ba43-b638e458cade</id><infoBaseAlias>DefAlias</infoBaseAlias>",
        "<targetType>ManagedClient</targetType></targetID><stateNum>16</stateNum>",
        "<state>Worked</state></debugRDBGRequestResponse:item></response>",
    );

    #[test]
    fn get_dbg_targets_shape_wraps_each_target_in_an_element_named_id() {
        let targets = parse_targets(GET_DBG_TARGETS.as_bytes());
        assert_eq!(targets.len(), 2, "two targets, got {targets:?}");
        assert_eq!(targets[0].id, "03432706-965e-4885-90d4-e179f5a08062");
        assert_eq!(targets[0].target_type, "ServerEmulation");
        assert_eq!(targets[0].infobase_alias, "DefAlias");
        assert_eq!(targets[0].seq_no, "2", "seanceNo stands in for the absent seqNo");
        assert_eq!(targets[1].id, "f79f9782-45e7-4241-ba43-b638e458cade");
        assert_eq!(targets[1].target_type, "ManagedClient");
    }

    #[test]
    fn step_shape_nests_the_fields_inside_target_id_and_adds_the_run_state() {
        let targets = parse_targets(STEP_RESPONSE.as_bytes());
        assert_eq!(targets.len(), 2, "two targets, got {targets:?}");
        assert_eq!(targets[0].id, "78791c83-e256-4569-be28-8379e7c77de8");
        assert_eq!(targets[0].target_type, "ServerEmulation");
        assert_eq!(targets[0].infobase_alias, "DefAlias");
        assert_eq!(targets[0].state, "Worked");
        assert_eq!(targets[1].target_type, "ManagedClient");
    }

    #[test]
    fn all_target_states_carry_the_run_state_of_each_target() {
        let targets = parse_targets(recorded::ALL_TARGET_STATES_WITH_A_HALTED_TARGET.as_bytes());
        assert_eq!(targets.len(), 2, "two targets, got {targets:?}");
        assert_eq!(targets[0].state, "Worked");
        assert!(!is_suspended_state(&targets[0].state), "a running target is not halted");
        assert_eq!(targets[1].id, "2a887b1e-1151-46c8-ab56-e0078edee53d");
        assert_eq!(targets[1].target_type, "ServerEmulation");
        assert_eq!(targets[1].state, "StopOnNextLine");
        assert!(is_suspended_state(&targets[1].state), "a breakpoint halt reads StopOnNextLine");
    }

    #[test]
    fn suspended_states_are_the_halted_ones_only() {
        for state in ["StopOnNextLine", "Evaluating"] {
            assert!(is_suspended_state(state), "{state} is halted");
        }
        for state in ["Worked", "WaitDebugger", "Registered", "NotRegistered", "Terminating", ""] {
            assert!(!is_suspended_state(state), "{state} is not a halt a Continue can resume");
        }
    }

    #[test]
    fn get_call_stack_answer_gives_the_halted_position() {
        let frames = parse_call_stack(recorded::GET_CALL_STACK.as_bytes());
        assert_eq!(frames.len(), 1, "one frame, got {frames:?}");
        assert_eq!(frames[0].line_no, 4);
        assert_eq!(frames[0].module_object_id, "5b0c1d3e-7a01-4c11-9a01-000000000001");
        assert_eq!(frames[0].module_property_id, "d5963243-262e-4398-b4d7-fb16d06484f6");
        // Unlike the stop event, this answer leaves the argument values out of the presentation.
        assert_eq!(frames[0].presentation, "ОбщийМодуль.ТестОтладки.Модуль.Посчитать(Н = )");
    }

    #[test]
    fn get_call_stack_with_no_content_has_no_frames() {
        assert!(parse_call_stack(b"").is_empty(), "HTTP 204 carries no body and no frames");
    }

    #[test]
    fn an_empty_answer_is_a_bare_response_element() {
        let targets = parse_targets(br#"<response xmlns="urn:x"/>"#);
        assert!(targets.is_empty(), "a bare <response/> holds no targets: {targets:?}");
    }

    #[test]
    fn a_failed_evaluation_carries_the_platform_error_text() {
        let xml = concat!(
            r#"<response xmlns:debugCalculations="http://v8.1c.ru/8.3/debugger/debugCalculations">"#,
            "<debugCalculations:evalResultState>withErrors</debugCalculations:evalResultState>",
            "<debugCalculations:expressionResultID>4c174628</debugCalculations:expressionResultID>",
            "<debugCalculations:resultValueInfo><debugCalculations:presProcessedCorrectly>false",
            "</debugCalculations:presProcessedCorrectly></debugCalculations:resultValueInfo>",
            "<debugCalculations:errorOccurred>true</debugCalculations:errorOccurred>",
            // {(1,1)}: Переменная не определена (НесуществующаяПеременная)
            "<debugCalculations:exceptionStr>eygxLDEpfTog0J/QtdGA0LXQvNC10L3QvdCw0Y8g0L3QtSDQvtC/\n",
            "0YDQtdC00LXQu9C10L3QsCAo0J3QtdGB0YPRidC10YHRgtCy0YPRjtGJ0LDRj9Cf0LXRgNC10LzQtdC90L3QsNGP",
            "KQ==</debugCalculations:exceptionStr></response>",
        );
        let payload = parse_eval_result(xml.as_bytes());
        let error = payload.error.expect("withErrors must produce an error");
        assert!(
            error.contains("Переменная не определена"),
            "the platform text must survive base64 and line wrapping: {error}"
        );
    }

    #[test]
    fn a_successful_evaluation_carries_one_value_and_no_error() {
        let xml = concat!(
            r#"<response xmlns:debugCalculations="http://v8.1c.ru/8.3/debugger/debugCalculations">"#,
            "<debugCalculations:evalResultState>correctly</debugCalculations:evalResultState>",
            "<debugCalculations:expressionResultID>2eea4a5a</debugCalculations:expressionResultID>",
            "<debugCalculations:resultValueInfo><debugCalculations:typeCode>4",
            "</debugCalculations:typeCode><debugCalculations:typeName>Строка",
            "</debugCalculations:typeName><debugCalculations:valueString>MjAyNg==",
            "</debugCalculations:valueString><debugCalculations:pres>IjIwMjYi",
            "</debugCalculations:pres></debugCalculations:resultValueInfo></response>",
        );
        let payload = parse_eval_result(xml.as_bytes());
        assert!(payload.error.is_none(), "a correct result is not an error: {payload:?}");
        assert_eq!(payload.values.len(), 1, "one value, got {:?}", payload.values);
        assert_eq!(payload.values[0].type_name, "Строка");
        assert_eq!(payload.values[0].presentation, "\"2026\"");
    }

    #[test]
    fn exception_descr_stays_on_one_line_in_both_recorded_shapes() {
        let unprefixed = concat!(
            r#"<exception xmlns="http://v8.1c.ru/8.2/virtual-resource-system" reason="400">"#,
            r#"<descr xmlns="http://v8.1c.ru/8.1/data/core">"#,
            "Ошибка при выполнении запроса POST к ресурсу /e1crdbg/rdbg:\n",
            "Выполнение вычислений возможно только в остановленном предмете отладки",
            "</descr></exception>",
        );
        let message = exception_descr(unprefixed.as_bytes()).expect("descr without a prefix");
        assert!(!message.contains('\n'), "must be one line: {message}");
        assert!(
            message.contains("rdbg:; Выполнение вычислений"),
            "the embedded newline becomes a separator: {message}"
        );

        let prefixed = concat!(
            r#"<exception xmlns:d1p1="urn:x"><d1p1:descr>Ошибка преобразования данных XDTO:"#,
            "\nНачалоСвойства: {ns}breakOnCondition\tФорма: Элемент",
            "</d1p1:descr><d1p1:data>junk</d1p1:data></exception>",
        );
        let message = exception_descr(prefixed.as_bytes()).expect("d1p1:descr");
        assert!(!message.contains('\t'), "the tab must go too: {message}");
        assert!(
            message.contains("breakOnCondition"),
            "the marker the version negotiation keys on must survive: {message}"
        );
    }

    #[test]
    fn a_body_without_a_descr_element_has_no_message() {
        assert!(exception_descr(b"<html>Bad Request</html>").is_none());
    }
}
