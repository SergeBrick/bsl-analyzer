//! The session against a scripted debug server, through the public API only: what the caller is
//! told must be what the server answered.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bsl_debug::session::{DebugConfig, DebugSession};

const SERVER_TARGET: &str = "2a887b1e-1151-46c8-ab56-e0078edee53d";
const MODULE_OBJECT: &str = "5b0c1d3e-7a01-4c11-9a01-000000000001";
const MODULE_PROPERTY: &str = "d5963243-262e-4398-b4d7-fb16d06484f6";
/// `ОбщийМодуль.ОбщийМодуль1.Модуль.Процедура1()`, base64 as the server sends it.
const PRESENTATION: &str =
    "0J7QsdGJ0LjQudCc0L7QtNGD0LvRjC7QntCx0YnQuNC50JzQvtC00YPQu9GMMS7QnNC+0LTRg9C70Ywu0J/RgNC+0YbQtdC00YPRgNCwMSgp";

const REGISTERED: &str = concat!(
    r#"<response xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
    "<debugRDBGRequestResponse:result>registered</debugRDBGRequestResponse:result></response>"
);

/// The 8.3.17 answer to a `setBreakpoints` that carries `breakOnCondition`, shortened.
const BREAK_ON_CONDITION_REFUSED: &str = concat!(
    r#"<exception xmlns:d1p1="http://v8.1c.ru/8.1/data/core"><d1p1:descr>Ошибка преобразования данных XDTO:"#,
    "\nНачалоСвойства: {http://v8.1c.ru/8.3/debugger/debugBreakpoints}breakOnCondition\tФорма: Элемент",
    "</d1p1:descr></exception>"
);

const ANY_REQUEST_REFUSED: &str = concat!(
    r#"<exception xmlns="http://v8.1c.ru/8.2/virtual-resource-system" reason="400">"#,
    r#"<descr xmlns="http://v8.1c.ru/8.1/data/core">Ошибка при выполнении запроса POST к ресурсу /e1crdbg/rdbg:"#,
    "\nUI+ - часть отладки не зарегистрирована</descr></exception>"
);

type Script = dyn Fn(&str, &str) -> (u16, String) + Send + Sync;

/// A debug server that answers by script and records every request but the event polls.
struct FakeServer {
    port: u16,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeServer {
    fn start(script: Box<Script>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script: Arc<Script> = Arc::from(script);
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let script = script.clone();
                let log = log.clone();
                std::thread::spawn(move || serve(stream, &*script, &log));
            }
        });
        Self { port, requests }
    }

    fn bodies_of(&self, cmd: &str) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| c == cmd)
            .map(|(_, b)| b.clone())
            .collect()
    }
}

fn serve(stream: std::net::TcpStream, script: &Script, log: &Mutex<Vec<(String, String)>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).to_string();
    let cmd = request_line
        .split("cmd=")
        .nth(1)
        .map(|rest| rest.split(['&', ' ']).next().unwrap_or("").to_string())
        .unwrap_or_default();
    if cmd == "pingDebugUIParams" {
        std::thread::sleep(Duration::from_millis(20));
    } else {
        log.lock().unwrap().push((cmd.clone(), body.clone()));
    }
    let (status, answer) = script(&cmd, &body);
    let reason = if status < 300 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{answer}",
        answer.len()
    );
    let mut stream = stream;
    let _ = stream.write_all(response.as_bytes());
}

fn quiet_server(cmd: &str, _body: &str) -> (u16, String) {
    match cmd {
        "attachDebugUI" => (200, REGISTERED.into()),
        "pingDebugUIParams" => (204, String::new()),
        _ => (200, String::new()),
    }
}

fn config_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Configuration.xml",
        "<?xml version=\"1.0\"?><MetaDataObject><Configuration \
         uuid=\"00000000-0000-0000-0000-0000000000c0\"></Configuration></MetaDataObject>",
    );
    write(
        "CommonModules/ОбщийМодуль1.xml",
        &format!(
            "<?xml version=\"1.0\"?><MetaDataObject><CommonModule \
             uuid=\"{MODULE_OBJECT}\"></CommonModule></MetaDataObject>"
        ),
    );
    write("CommonModules/ОбщийМодуль1/Ext/Module.bsl", "Процедура Процедура1() Экспорт\n");
    dir
}

fn connect(server: &FakeServer, root: &Path) -> DebugSession {
    DebugSession::connect(DebugConfig {
        host: "127.0.0.1".into(),
        port: server.port,
        infobase: "DefAlias".into(),
        config_root: root.to_path_buf(),
        extensions: Vec::new(),
        auto_attach: Vec::new(),
    })
    .expect("attach")
}

/// One ping answer carrying a single event of the given type and body.
fn ping_event(kind: &str, inner: &str) -> String {
    format!(
        concat!(
            r#"<response xmlns="http://v8.1c.ru/8.3/debugger/debugBaseData" "#,
            r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" "#,
            r#"xmlns:debugRDBGRequestResponse="http://v8.1c.ru/8.3/debugger/debugRDBGRequestResponse">"#,
            r#"<debugRDBGRequestResponse:result xsi:type="debugRDBGRequestResponse:{kind}">"#,
            "{inner}</debugRDBGRequestResponse:result></response>"
        ),
        kind = kind,
        inner = inner,
    )
}

fn stop_event() -> String {
    ping_event(
        "DBGUIExtCmdInfoCallStackFormed",
        &format!(
            concat!(
                "<cmdID>callStackFormed</cmdID><targetID><id>{target}</id>",
                "<targetType>ServerEmulation</targetType></targetID>",
                "<callStack><moduleID><objectID>{object}</objectID>",
                "<propertyID>{property}</propertyID></moduleID><lineNo>1</lineNo>",
                "<presentation>{pres}</presentation></callStack><stopByBP>true</stopByBP>"
            ),
            target = SERVER_TARGET,
            object = MODULE_OBJECT,
            property = MODULE_PROPERTY,
            pres = PRESENTATION,
        ),
    )
}

#[test]
fn a_refused_breakpoint_list_is_reported_with_the_server_text() {
    let server = FakeServer::start(Box::new(|cmd: &str, body: &str| {
        if cmd == "setBreakpoints" {
            (400, ANY_REQUEST_REFUSED.into())
        } else {
            quiet_server(cmd, body)
        }
    }));
    let root = config_root();
    let mut session = connect(&server, root.path());

    let result = session.set_breakpoint("ОбщийМодуль.ОбщийМодуль1", 1, None);
    assert!(!server.bodies_of("setBreakpoints").is_empty(), "the request was sent");
    let err = match result {
        Err(e) => e.to_string(),
        Ok(()) => panic!("HTTP 400 must not be reported as a breakpoint that was set"),
    };
    assert!(err.contains("часть отладки не зарегистрирована"), "{err}");
}

#[test]
fn a_conditional_breakpoint_refused_over_break_on_condition_is_resent_without_it() {
    let server = FakeServer::start(Box::new(|cmd: &str, body: &str| {
        if cmd == "setBreakpoints" && body.contains("breakOnCondition") {
            (400, BREAK_ON_CONDITION_REFUSED.into())
        } else {
            quiet_server(cmd, body)
        }
    }));
    let root = config_root();
    let mut session = connect(&server, root.path());

    session
        .set_breakpoint("ОбщийМодуль.ОбщийМодуль1", 1, Some("Истина"))
        .expect("the conditional breakpoint is accepted in the shape this server takes");
    let sent = server.bodies_of("setBreakpoints");
    let accepted = sent.last().expect("a setBreakpoints request");
    assert!(
        !accepted.contains("breakOnCondition") && accepted.contains("Истина"),
        "the last request keeps the condition without breakOnCondition: {sent:#?}"
    );

    session.set_breakpoint("ОбщийМодуль.ОбщийМодуль1", 1, Some("Ложь")).expect("second");
    let after = server.bodies_of("setBreakpoints");
    assert_eq!(after.len(), sent.len() + 1, "no second refusal to negotiate: {after:#?}");
}

#[test]
fn an_evaluation_error_carries_the_platform_text() {
    let result_id = Arc::new(Mutex::new(None::<String>));
    let stop_sent = Arc::new(Mutex::new(false));
    let server = FakeServer::start(Box::new(move |cmd: &str, body: &str| match cmd {
        "pingDebugUIParams" => {
            let mut stop_sent = stop_sent.lock().unwrap();
            if !*stop_sent {
                *stop_sent = true;
                return (200, stop_event());
            }
            match result_id.lock().unwrap().take() {
                // {(1,1)}: Переменная не определена (НесуществующаяПеременная)
                Some(id) => (
                    200,
                    ping_event(
                        "DBGUIExtCmdInfoExprEvaluated",
                        &format!(
                            concat!(
                                "<cmdID>exprEvaluated</cmdID><targetID><id>{target}</id></targetID>",
                                "<evalExprResult><evalResultState>withErrors</evalResultState>",
                                "<expressionResultID>{id}</expressionResultID>",
                                "<errorOccurred>true</errorOccurred><exceptionStr>",
                                "eygxLDEpfTog0J/QtdGA0LXQvNC10L3QvdCw0Y8g0L3QtSDQvtC/\n",
                                "0YDQtdC00LXQu9C10L3QsCAo0J3QtdGB0YPRidC10YHRgtCy0YPRjtGJ0LDRj9Cf0LXRgNC10LzQtdC90L3QsNGP",
                                "KQ==</exceptionStr></evalExprResult>"
                            ),
                            target = SERVER_TARGET,
                            id = id,
                        ),
                    ),
                ),
                None => (204, String::new()),
            }
        }
        "evalExpr" => {
            let id = body
                .split("expressionResultID>")
                .nth(1)
                .and_then(|rest| rest.split('<').next())
                .map(str::to_string);
            *result_id.lock().unwrap() = id;
            (200, String::new())
        }
        _ => quiet_server(cmd, body),
    }));
    let root = config_root();
    let mut session = connect(&server, root.path());

    let stop = session.wait_for_stop(Duration::from_secs(5)).unwrap().expect("the stop");
    assert_eq!(stop.target_id, SERVER_TARGET);

    match session.eval("НесуществующаяПеременная", 0) {
        Err(e) => {
            let text = e.to_string();
            assert!(text.contains("Переменная не определена"), "{text}");
        }
        Ok(value) => panic!("an evaluation error must not read as a value: {value:?}"),
    }
}
