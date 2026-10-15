use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};
use tokio::process::{Child as TokioChild, Command as TokioCommand};

use super::args;

type Client = RunningService<RoleClient, ()>;

fn metric_binary_path() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            let path = std::env::var_os("BSL_ANALYZER_BENCH_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_bsl-analyzer-app")));
            path.canonicalize().expect("resolve shared MCP/LSP measurement binary")
        })
        .as_path()
}

const FIXTURE_CALLER: &str = "CommonModules/Потребитель/Ext/Module.bsl";
const FIXTURE_TARGET: &str = "CommonModules/ОбщийМодуль/Ext/Module.bsl";
const PERF_SHADOW_MANIFEST_HASH: &str =
    "e3a61e08c4c48841da096794968ddf04959e096bbfcc38b516ec64c74c14f574";
const CORPUS_CALLER: &str = "CommonModules/Module0001/Ext/Module.bsl";
const CORPUS_TARGET: &str = "CommonModules/Module0999/Ext/Module.bsl";
const SCOPE_CALLER: &str = "CommonModules/ScopePerfCaller/Ext/Module.bsl";
const CORPUS_MANIFEST_HASH: &str =
    "b3dbad0f1053db327ef18840db3388bb31beb50205e3b4f7d2dccc42a95df7c9";
const METRIC_CYCLES: usize = 30;
const RUNS_PER_GROUP: usize = 5;

fn require_release_profile(debug_assertions: bool) {
    assert!(!debug_assertions, "MCP workspace metrics require a release-profile binary");
}

#[test]
fn measurement_profile_guard_accepts_release_and_rejects_debug() {
    require_release_profile(false);
    assert!(std::panic::catch_unwind(|| require_release_profile(true)).is_err());
}

struct GroupSelection {
    keys: BTreeSet<String>,
    requested: String,
    partial: bool,
}

impl GroupSelection {
    fn from_env() -> Self {
        Self::parse(&std::env::var("BSL_ANALYZER_BENCH_GROUPS").unwrap_or_default())
    }

    fn parse(requested: &str) -> Self {
        let requested = requested.to_owned();
        let partial = !requested.trim().is_empty();
        let mut keys = BTreeSet::new();
        if partial {
            for selector in requested.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                let parts: Vec<_> = selector.split('/').collect();
                assert_eq!(parts.len(), 3, "invalid benchmark group selector: {selector}");
                assert!(
                    matches!(parts[0], "perf-shadow" | "corpus-1000-v4"),
                    "unknown benchmark dataset in selector: {selector}"
                );
                assert!(
                    matches!(parts[1], "cold" | "warm" | "scope"),
                    "unknown benchmark group in selector: {selector}"
                );
                let (first, last) = if let Some((first, last)) = parts[2].split_once('-') {
                    (
                        first
                            .parse::<usize>()
                            .unwrap_or_else(|_| panic!("invalid group range: {selector}")),
                        last.parse::<usize>()
                            .unwrap_or_else(|_| panic!("invalid group range: {selector}")),
                    )
                } else {
                    let index = parts[2]
                        .parse::<usize>()
                        .unwrap_or_else(|_| panic!("invalid group range: {selector}"));
                    (index, index)
                };
                assert!(
                    first > 0 && first <= last && last <= RUNS_PER_GROUP,
                    "group range must be within 1..={RUNS_PER_GROUP}: {selector}"
                );
                for index in first..=last {
                    keys.insert(format!("{}/{}/{index}", parts[0], parts[1]));
                }
            }
            assert!(!keys.is_empty(), "benchmark group selection is empty");
        } else {
            for dataset in ["perf-shadow", "corpus-1000-v4"] {
                for group in ["cold", "warm", "scope"] {
                    for index in 1..=RUNS_PER_GROUP {
                        keys.insert(format!("{dataset}/{group}/{index}"));
                    }
                }
            }
        }
        Self { keys, requested, partial }
    }

    fn contains(&self, dataset: &str, group: &str, index: usize) -> bool {
        self.keys.contains(&format!("{dataset}/{group}/{index}"))
    }

    fn count(&self, group: &str, dataset: Option<&str>) -> usize {
        self.keys
            .iter()
            .filter(|key| {
                let mut parts = key.split('/');
                let key_dataset = parts.next().unwrap_or_default();
                let key_group = parts.next().unwrap_or_default();
                key_group == group && dataset.is_none_or(|expected| key_dataset == expected)
            })
            .count()
    }

    fn labels(&self) -> Vec<&str> {
        self.keys.iter().map(String::as_str).collect()
    }
}

#[test]
fn group_selection_covers_the_requested_partial_and_full_sets() {
    let selected = GroupSelection::parse(
        "perf-shadow/scope/2-5;corpus-1000-v4/cold/1-5;corpus-1000-v4/warm/1-5;corpus-1000-v4/scope/1-5",
    );
    assert!(selected.partial);
    assert_eq!(selected.keys.len(), 19);
    assert_eq!(selected.count("scope", Some("perf-shadow")), 4);
    assert_eq!(selected.count("cold", Some("corpus-1000-v4")), 5);
    assert_eq!(selected.count("warm", Some("corpus-1000-v4")), 5);
    assert_eq!(selected.count("scope", Some("corpus-1000-v4")), 5);
    assert!(!selected.contains("perf-shadow", "cold", 1));
    assert!(!selected.contains("perf-shadow", "warm", 5));
    assert!(!selected.contains("perf-shadow", "scope", 1));

    let full = GroupSelection::parse("");
    assert!(!full.partial);
    assert_eq!(full.keys.len(), 30);
    assert!(full.contains("perf-shadow", "scope", 1));
    assert!(full.contains("corpus-1000-v4", "warm", 5));

    assert!(std::panic::catch_unwind(|| GroupSelection::parse("unknown/scope/1-5")).is_err());
    assert!(std::panic::catch_unwind(|| GroupSelection::parse("perf-shadow/scope/1-6")).is_err());
}

#[derive(Clone, Copy)]
struct Dataset {
    name: &'static str,
    caller_path: &'static str,
    symbol: &'static str,
    global_symbol: Option<&'static str>,
    global_xml_path: &'static str,
    global_bare_call: &'static str,
    diagnostic_name: &'static str,
    target_path: &'static str,
    target_function: &'static str,
}

const FIXTURE_DATASET: Dataset = Dataset {
    name: "perf-shadow",
    caller_path: FIXTURE_CALLER,
    symbol: "ОбщийМодуль.ЧисловоеЗначение",
    global_symbol: None,
    global_xml_path: "CommonModules/ОбщийМодуль.xml",
    global_bare_call: "ЧисловоеЗначение",
    diagnostic_name: "НетТакойПроцедуры",
    target_path: FIXTURE_TARGET,
    target_function: "ЧисловоеЗначение",
};

const CORPUS_DATASET: Dataset = Dataset {
    name: "corpus-1000-v4",
    caller_path: CORPUS_CALLER,
    symbol: "Module0999.Значение",
    global_symbol: Some("Module0000.BenchmarkGlobalCall"),
    global_xml_path: "CommonModules/Module0000.xml",
    global_bare_call: "BenchmarkGlobalCall",
    diagnostic_name: "НетТакойПроцедуры",
    target_path: CORPUS_TARGET,
    target_function: "Значение",
};

#[derive(serde::Serialize)]
struct ProcessRun {
    dataset: &'static str,
    dataset_manifest_sha256: Option<&'static str>,
    cache: &'static str,
    run: usize,
    mcp_pid: u32,
    lsp_pid: u32,
    lazy_control_requests_us: u128,
    process_to_handshake_us: u128,
    explicit_diagnostics_status_us: u128,
    explicit_graph_status_us: u128,
    diagnostics_status_after_static_controls: Value,
    graph_status_after_static_controls: Value,
    handshake_to_resident_result_us: u128,
    handshake_to_graph_ready_us: u128,
    graph_overview_us: u128,
    lsp_initialize_and_open_us: u128,
    mcp_hwm_kib_before_cleanup: u64,
    lsp_hwm_kib_before_cleanup: u64,
    rss: Vec<PairRssSample>,
}

#[derive(Clone, serde::Serialize)]
struct PairRssSample {
    elapsed_us: u128,
    sequential_read_skew_us: u128,
    mcp_rss_kib: u64,
    mcp_hwm_kib: u64,
    lsp_rss_kib: u64,
    lsp_hwm_kib: u64,
    pair_rss_kib: u64,
}

#[derive(serde::Serialize)]
struct LatencySample {
    dataset: &'static str,
    cache_group: &'static str,
    run: usize,
    kind: &'static str,
    iteration: usize,
    expected: String,
    elapsed_us: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    lsp_client_poll_attempts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lsp_content_modified_interruptions: Option<usize>,
}

#[derive(serde::Serialize)]
struct ScopeRun {
    dataset: &'static str,
    group: usize,
    cycle: usize,
    excluded_module: String,
    old_mcp_pid: u32,
    new_mcp_pid: u32,
    lsp_pid: u32,
    old_scope_symbol: String,
    new_scope_symbol_status: String,
    retained_symbol_status: String,
    global_symbol_status: Option<String>,
    lsp_old_composition_poll_attempts: usize,
    lsp_old_composition_content_modified_interruptions: usize,
    lsp_new_composition_poll_attempts: usize,
    lsp_new_composition_content_modified_interruptions: usize,
    config_to_old_mcp_retired_us: u128,
    config_to_lsp_new_composition_us: u128,
    config_to_mcp_reconnected_us: u128,
    old_mcp_stderr: String,
    reconnected_mcp_stderr: String,
    old_pair_rss: Vec<PairRssSample>,
    reconnected_pair_rss: Vec<PairRssSample>,
}

struct DefinitionPoll {
    elapsed_us: u128,
    attempts: usize,
    content_modified_interruptions: usize,
}

#[derive(serde::Serialize)]
struct EffectiveSettingsRun {
    dataset: &'static str,
    dataset_manifest_sha256: &'static str,
    cache_group: &'static str,
    run: usize,
    cycle: usize,
    mcp_pid: u32,
    lsp_pid: u32,
    diagnostic_code: &'static str,
    diagnostic_name: &'static str,
    retained_symbol: &'static str,
    retained_symbol_path: String,
    source_identity_unchanged: bool,
    config_to_disabled_absent_us: u128,
    config_to_restored_present_us: u128,
}

struct McpChild {
    process: TokioChild,
    client: Client,
    stderr: Arc<Mutex<String>>,
    stderr_thread: Option<tokio::task::JoinHandle<()>>,
    pid: u32,
}

impl McpChild {
    async fn start(workspace: &Path, cache: &Path) -> Self {
        std::fs::create_dir_all(cache).expect("owned MCP cache directory");
        let mut process = TokioCommand::new(metric_binary_path())
            .args(["mcp", "serve", "--profile", "workspace", "--mode", "stdio", "-s"])
            .arg(workspace)
            .arg("--cache-dir")
            .arg(cache)
            .current_dir(workspace)
            .env("XDG_CONFIG_HOME", cache.join("xdg-config"))
            .env("XDG_CACHE_HOME", cache.join("xdg-cache"))
            .env("XDG_STATE_HOME", cache.join("xdg-state"))
            .env("XDG_DATA_HOME", cache.join("xdg-data"))
            .env_remove("BSL_CACHE_DIR")
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("BSL_PROFILE")
            .env_remove("BSL_LOG_FILE")
            .env_remove("BSL_MCP_DAEMON_LOG")
            .env_remove("EMBEDDING_URL")
            .env_remove("EMBEDDING_MODEL")
            .env_remove("EMBEDDING_DIM")
            .env_remove("EMBEDDING_API_KEY")
            .env_remove("EMBEDDING_PROVIDER")
            .env_remove("EMBEDDING_QUERY_PREFIX")
            .env_remove("EMBEDDING_DOCUMENT_PREFIX")
            .kill_on_drop(true)
            .env("BSL_MCP_BROKER", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start direct stdio MCP backend");
        let pid = process.id().expect("owned MCP child PID");
        let stdout = process.stdout.take().expect("MCP stdout pipe");
        let stdin = process.stdin.take().expect("MCP stdin pipe");
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_thread = {
            let stderr_text = stderr.clone();
            let mut reader =
                tokio::io::BufReader::new(process.stderr.take().expect("MCP stderr pipe"));
            tokio::spawn(async move {
                let mut line = String::new();
                while tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
                    .await
                    .unwrap_or(0)
                    > 0
                {
                    stderr_text.lock().expect("MCP stderr lock").push_str(&line);
                    line.clear();
                }
            })
        };
        let client = ().serve((stdout, stdin)).await.expect("MCP initialize handshake");
        Self { process, client, stderr, stderr_thread: Some(stderr_thread), pid }
    }

    async fn stop(mut self) {
        drop(self.client);
        if self.process.try_wait().expect("check MCP child").is_none() {
            let _ = self.process.kill().await;
        }
        let _ = self.process.wait().await;
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.await;
        }
    }
}

struct OwnedLsp {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: u64,
    version: i32,
}

impl OwnedLsp {
    fn start(workspace: &Path, cache: &Path, caller: &str, caller_text: &str) -> Self {
        Self::start_inner(workspace, cache, caller, caller_text, None).0
    }

    fn start_with_pair_sampler(
        workspace: &Path,
        cache: &Path,
        caller: &str,
        caller_text: &str,
        mcp_pid: u32,
    ) -> (Self, PairSampler) {
        let (lsp, sampler) =
            Self::start_inner(workspace, cache, caller, caller_text, Some(mcp_pid));
        (lsp, sampler.expect("paired sampler for launched MCP/LSP processes"))
    }

    fn start_inner(
        workspace: &Path,
        cache: &Path,
        caller: &str,
        caller_text: &str,
        mcp_pid: Option<u32>,
    ) -> (Self, Option<PairSampler>) {
        std::fs::create_dir_all(cache).expect("owned LSP cache directory");
        let mut child = Command::new(metric_binary_path())
            .arg("lsp")
            .current_dir(workspace)
            .env("XDG_CONFIG_HOME", cache.join("lsp-xdg-config"))
            .env("XDG_CACHE_HOME", cache.join("lsp-xdg-cache"))
            .env("XDG_STATE_HOME", cache.join("lsp-xdg-state"))
            .env("XDG_DATA_HOME", cache.join("lsp-xdg-data"))
            .env_remove("BSL_CACHE_DIR")
            .env_remove("BSL_PROFILE")
            .env_remove("BSL_LOG")
            .env_remove("BSL_LOG_FILE")
            .env_remove("XDG_RUNTIME_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start owned Content-Length LSP peer");
        let sampler = mcp_pid.map(|pid| PairSampler::start(pid, child.id()));
        let stdin = child.stdin.take().expect("LSP stdin");
        let stdout = child.stdout.take().expect("LSP stdout");
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut content_length = None;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).ok().filter(|length| *length > 0).is_none() {
                        return;
                    }
                    if header == "\r\n" {
                        break;
                    }
                    if let Some(value) = header.strip_prefix("Content-Length:") {
                        content_length = value.trim().parse::<usize>().ok();
                    }
                }
                let Some(content_length) = content_length else { return };
                let mut body = vec![0; content_length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if let Ok(message) = serde_json::from_slice(&body) {
                    if sender.send(message).is_err() {
                        return;
                    }
                }
            }
        });

        let mut lsp = Self { child, stdin, messages, next_id: 2, version: 1 };
        let root_uri = lsp_types::Url::from_directory_path(workspace).expect("workspace URI");
        lsp.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"rootUri": root_uri, "capabilities": {}}
        }));
        let initialized = lsp.wait_response(1);
        assert!(initialized["error"].is_null(), "LSP initialize failed: {initialized}");
        lsp.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        let uri = lsp_types::Url::from_file_path(workspace.join(caller)).expect("caller URI");
        lsp.send(json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": uri.clone(), "languageId": "bsl", "version": 1, "text": caller_text}}
        }));
        let published = lsp.wait_message(Duration::from_secs(180), |message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri.as_str()
        });
        assert!(
            published["params"]["diagnostics"].as_array().is_some_and(|items| {
                items.iter().any(|diagnostic| diagnostic["code"] == "UnresolvedBareCall")
            }),
            "LSP peer did not publish its known diagnostic: {published}"
        );
        (lsp, sampler)
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .expect("write LSP Content-Length frame");
        self.stdin.flush().expect("flush LSP frame");
    }

    fn wait_message(&self, timeout: Duration, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "LSP Content-Length response timed out");
            let message = self.messages.recv_timeout(remaining).expect("LSP response channel");
            if predicate(&message) {
                return message;
            }
        }
    }

    fn wait_response(&self, id: u64) -> Value {
        self.wait_message(Duration::from_secs(180), |message| {
            message["id"].as_u64() == Some(id) && message["method"].is_null()
        })
    }

    fn wait_response_until(&self, id: u64, deadline: Instant) -> Value {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "LSP response {id} exceeded its polling deadline");
            let message = self
                .messages
                .recv_timeout(remaining)
                .unwrap_or_else(|error| panic!("LSP response {id} timed out: {error}"));
            if message["id"].as_u64() == Some(id) && message["method"].is_null() {
                return message;
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait_response(id)
    }

    fn request_until(&mut self, method: &str, params: Value, deadline: Instant) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait_response_until(id, deadline)
    }

    fn set_caller_text(&mut self, workspace: &Path, caller: &str, text: &str) {
        self.version += 1;
        let uri = lsp_types::Url::from_file_path(workspace.join(caller)).expect("caller URI");
        self.send(json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {"textDocument": {"uri": uri, "version": self.version}, "contentChanges": [{"text": text}]}
        }));
    }

    fn watched_config(&mut self, workspace: &Path) {
        let uri = lsp_types::Url::from_file_path(workspace.join("bsl-analyzer.toml"))
            .expect("project config URI");
        self.send(json!({
            "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
            "params": {"changes": [{"uri": uri, "type": 2}]}
        }));
    }

    fn definition_at_until(
        &mut self,
        caller: &str,
        workspace: &Path,
        line: u64,
        character: u64,
        deadline: Instant,
    ) -> Value {
        let uri = lsp_types::Url::from_file_path(workspace.join(caller)).expect("caller URI");
        self.request_until(
            "textDocument/definition",
            json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}}),
            deadline,
        )
    }

    fn wait_definition_path_at(
        &mut self,
        caller: &str,
        workspace: &Path,
        line: u64,
        module: &str,
    ) -> DefinitionPoll {
        let started = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut attempts = 0;
        let mut content_modified_interruptions = 0;
        loop {
            attempts += 1;
            let result = self.definition_at_until(caller, workspace, line, 4, deadline);
            if result["error"]["code"].as_i64() == Some(-32801) {
                content_modified_interruptions += 1;
                assert!(
                    Instant::now() < deadline,
                    "LSP definition stayed ContentModified for {attempts} attempts: {result}"
                );
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            assert!(result["error"].is_null(), "LSP definition request failed: {result}");
            if result["result"].to_string().contains(module) {
                return DefinitionPoll {
                    elapsed_us: started.elapsed().as_micros(),
                    attempts,
                    content_modified_interruptions,
                };
            }
            assert!(
                Instant::now() < deadline,
                "LSP did not resolve current scope module: {result}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_definition_absent_at(
        &mut self,
        caller: &str,
        workspace: &Path,
        line: u64,
    ) -> DefinitionPoll {
        let started = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut attempts = 0;
        let mut content_modified_interruptions = 0;
        loop {
            attempts += 1;
            let result = self.definition_at_until(caller, workspace, line, 4, deadline);
            if result["error"]["code"].as_i64() == Some(-32801) {
                content_modified_interruptions += 1;
                assert!(
                    Instant::now() < deadline,
                    "LSP definition stayed ContentModified for {attempts} attempts: {result}"
                );
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            assert!(result["error"].is_null(), "LSP definition request failed: {result}");
            if result["result"].is_null()
                || result["result"].as_array().is_some_and(|locations| locations.is_empty())
            {
                return DefinitionPoll {
                    elapsed_us: started.elapsed().as_micros(),
                    attempts,
                    content_modified_interruptions,
                };
            }
            assert!(
                Instant::now() < deadline,
                "LSP retained a definition outside new scope: {result}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn shutdown(&mut self) {
        let response = self.request("shutdown", Value::Null);
        assert!(response["error"].is_null(), "LSP shutdown failed: {response}");
        self.send(json!({"jsonrpc": "2.0", "method": "exit"}));
        let _ = self.child.wait().expect("LSP exits after shutdown/exit");
    }
}

impl Drop for OwnedLsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct PairSampler {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<PairRssSample>>>,
    started: Instant,
    mcp_pid: u32,
    lsp_pid: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PairSampler {
    fn start(mcp_pid: u32, lsp_pid: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::new()));
        let worker_stop = stop.clone();
        let worker_samples = samples.clone();
        let started = Instant::now();
        let thread = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let pair_started = Instant::now();
                if let Some((mcp_rss_kib, mcp_hwm_kib)) = process_memory(mcp_pid) {
                    let mcp_sampled_at = Instant::now();
                    if let Some((lsp_rss_kib, lsp_hwm_kib)) = process_memory(lsp_pid) {
                        let sequential_read_skew_us = mcp_sampled_at.elapsed().as_micros();
                        worker_samples.lock().expect("RSS samples lock").push(PairRssSample {
                            elapsed_us: pair_started.duration_since(started).as_micros(),
                            sequential_read_skew_us,
                            mcp_rss_kib,
                            mcp_hwm_kib,
                            lsp_rss_kib,
                            lsp_hwm_kib,
                            pair_rss_kib: mcp_rss_kib + lsp_rss_kib,
                        });
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        Self { stop, samples, started, mcp_pid, lsp_pid, thread: Some(thread) }
    }

    fn finish(mut self) -> Vec<PairRssSample> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().expect("RSS sampler thread").join().expect("RSS sampler exits");
        let mut samples = self.samples.lock().expect("RSS samples lock").clone();
        let pair_started = Instant::now();
        if let Some((mcp_rss_kib, mcp_hwm_kib)) = process_memory(self.mcp_pid) {
            let mcp_sampled_at = Instant::now();
            if let Some((lsp_rss_kib, lsp_hwm_kib)) = process_memory(self.lsp_pid) {
                samples.push(PairRssSample {
                    elapsed_us: pair_started.duration_since(self.started).as_micros(),
                    sequential_read_skew_us: mcp_sampled_at.elapsed().as_micros(),
                    mcp_rss_kib,
                    mcp_hwm_kib,
                    lsp_rss_kib,
                    lsp_hwm_kib,
                    pair_rss_kib: mcp_rss_kib + lsp_rss_kib,
                });
            }
        }
        assert!(!samples.is_empty(), "both owned processes must have /proc VmRSS samples");
        samples
    }
}

impl Drop for PairSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn process_memory(pid: u32) -> Option<(u64, u64)> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let mut rss = None;
    let mut hwm = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            rss = value.split_whitespace().next()?.parse().ok();
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            hwm = value.split_whitespace().next()?.parse().ok();
        }
    }
    Some((rss?, hwm?))
}

fn dataset_manifest_sha256(dataset: Dataset) -> &'static str {
    if dataset.name == FIXTURE_DATASET.name {
        PERF_SHADOW_MANIFEST_HASH
    } else {
        CORPUS_MANIFEST_HASH
    }
}

fn has_known_diagnostic(result: &Value, dataset: Dataset) -> bool {
    has_unresolved_bare_call(result, dataset.diagnostic_name)
}

fn has_unresolved_bare_call(result: &Value, name: &str) -> bool {
    result["result"]["findings"].as_array().is_some_and(|findings| {
        findings.iter().any(|finding| {
            finding["code"] == "UnresolvedBareCall"
                && finding["message"].as_str().is_some_and(|message| message.contains(name))
        })
    })
}

async fn wait_diagnostic_present(client: &Client, dataset: Dataset) -> Value {
    wait_diagnostic_named_present(client, dataset, dataset.diagnostic_name).await
}

async fn wait_diagnostic_named_present(client: &Client, dataset: Dataset, name: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let result = call(
            client,
            "diagnostics",
            args(&[
                ("action", json!("file")),
                ("path", json!(dataset.caller_path)),
                ("min_severity", json!("hint")),
                ("detail", json!("detailed")),
            ]),
        )
        .await;
        if result["status"] != "loading" && has_unresolved_bare_call(&result, name) {
            return result;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "known {} diagnostic did not become present: {result}",
            name
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn perf_shadow_fixture() -> tempfile::TempDir {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source =
        repo.join("crates/bsl-analyzer/tests/fixtures/shared_workspace_analysis/perf-shadow");
    let manifest: Value =
        serde_json::from_slice(
            &std::fs::read(repo.join(
                ".artifacts/issue384-implementation/baseline/shared-perf-shadow-manifest.json",
            ))
            .expect("read pinned performance shadow manifest"),
        )
        .expect("parse performance shadow manifest");
    assert_eq!(manifest["workspace_sha256"], PERF_SHADOW_MANIFEST_HASH);

    let workspace = tempfile::tempdir().expect("performance shadow workspace");
    for entry in walkdir::WalkDir::new(&source) {
        let entry = entry.expect("performance shadow input entry");
        let relative = entry.path().strip_prefix(&source).expect("shadow relative path");
        let target = workspace.path().join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("create shadow workspace directory");
        } else {
            std::fs::create_dir_all(target.parent().expect("shadow parent"))
                .expect("create shadow parent");
            std::fs::copy(entry.path(), &target).expect("copy pinned performance shadow input");
        }
    }
    let files = manifest["files"].as_array().expect("performance shadow manifest files");
    assert_eq!(files.len(), 5, "pinned performance shadow file count");
    for file in files {
        let relative = file["path"].as_str().expect("shadow manifest path");
        let bytes = std::fs::read(workspace.path().join(relative)).expect("copied shadow file");
        assert_eq!(
            bytes.len(),
            file["bytes"].as_u64().unwrap() as usize,
            "shadow bytes: {relative}"
        );
    }
    workspace
}

fn corpus_1000_v4() -> tempfile::TempDir {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".artifacts/issue384-implementation/baseline/corpus-1000-v4");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(source.join("input-manifest.json")).expect("read pinned corpus manifest"),
    )
    .expect("parse pinned corpus manifest");
    assert_eq!(manifest["module_count"], 1000);
    assert_eq!(manifest["workspace_sha256"], CORPUS_MANIFEST_HASH);

    let workspace = tempfile::tempdir().expect("1000-module workspace");
    for entry in walkdir::WalkDir::new(&source)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "input-manifest.json")
    {
        let entry = entry.expect("corpus input entry");
        let relative = entry.path().strip_prefix(&source).expect("corpus relative path");
        let target = workspace.path().join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("create corpus directory");
        } else {
            std::fs::create_dir_all(target.parent().expect("corpus parent"))
                .expect("create corpus parent");
            std::fs::copy(entry.path(), target).expect("copy pinned corpus input");
        }
    }

    let listed = manifest["files"].as_array().expect("manifest file list");
    assert_eq!(listed.len(), 2002, "pinned v4 corpus manifest file count");
    for file in listed {
        let relative = file["path"].as_str().expect("manifest relative path");
        let bytes = std::fs::read(workspace.path().join(relative)).expect("copied corpus file");
        assert_eq!(
            bytes.len(),
            file["bytes"].as_u64().unwrap() as usize,
            "corpus bytes: {relative}"
        );
    }
    workspace
}

fn clone_workspace(source: &Path) -> tempfile::TempDir {
    let target = tempfile::tempdir().expect("cloned workspace");
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.expect("workspace clone entry");
        let relative = entry.path().strip_prefix(source).expect("workspace-relative path");
        let destination = target.path().join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&destination).expect("create cloned workspace directory");
        } else {
            std::fs::create_dir_all(destination.parent().expect("clone parent"))
                .expect("create cloned workspace parent");
            std::fs::copy(entry.path(), destination).expect("copy workspace input");
        }
    }
    target
}

fn reset_workspace(target: &Path, source: &Path) {
    for entry in std::fs::read_dir(target).expect("list measured workspace") {
        let path = entry.expect("measured workspace entry").path();
        if path.is_dir() {
            std::fs::remove_dir_all(path).expect("clear measured workspace directory");
        } else {
            std::fs::remove_file(path).expect("clear measured workspace file");
        }
    }
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry.expect("source reset entry");
        let relative = entry.path().strip_prefix(source).expect("reset relative path");
        let destination = target.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&destination).expect("create reset workspace directory");
        } else {
            std::fs::create_dir_all(destination.parent().expect("reset parent"))
                .expect("create reset parent");
            std::fs::copy(entry.path(), destination).expect("restore workspace input");
        }
    }
}

fn tree_blake3(root: &Path, excluded_file: Option<&str>) -> String {
    let mut files = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            excluded_file.is_none_or(|excluded| entry.file_name().to_str() != Some(excluded))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.path().to_path_buf());
    let mut hasher = blake3::Hasher::new();
    for entry in files {
        let relative = entry.path().strip_prefix(root).expect("tree-relative path");
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&std::fs::read(entry.path()).expect("read tree hash input"));
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}

fn actual_hashes(dataset: Dataset, input: &Path) -> Value {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = metric_binary_path();
    let fixture_manifest =
        repo.join(".artifacts/issue384-implementation/baseline/shared-perf-shadow-manifest.json");
    let corpus_manifest =
        repo.join(".artifacts/issue384-implementation/baseline/corpus-1000-v4/input-manifest.json");
    static SOURCE_HASH: OnceLock<String> = OnceLock::new();
    let source_hash = SOURCE_HASH.get_or_init(|| tree_blake3(&repo.join("crates"), None));
    let mut build_inputs = blake3::Hasher::new();
    for path in ["Cargo.toml", "Cargo.lock"] {
        build_inputs.update(path.as_bytes());
        build_inputs.update(&[0]);
        build_inputs.update(&std::fs::read(repo.join(path)).expect("read build input"));
        build_inputs.update(&[0]);
    }
    let harness_hash = [
        "crates/bsl-analyzer/tests/shared_workspace_analysis.rs",
        "crates/bsl-analyzer/tests/shared_workspace_analysis/mcp_metrics.rs",
    ]
    .into_iter()
    .map(|path| {
        let bytes = std::fs::read(repo.join(path)).expect("read actual harness source");
        blake3::hash(&bytes).to_hex().to_string()
    })
    .collect::<Vec<_>>();
    json!({
        "source_tree_blake3_crates": source_hash,
        "build_manifests_blake3": build_inputs.finalize().to_hex().to_string(),
        "harness_file_blake3": harness_hash,
        "binary_path": binary.display().to_string(),
        "binary_blake3": blake3::hash(&std::fs::read(binary).expect("read actual MCP/LSP binary")).to_hex().to_string(),
        "input_tree_blake3": tree_blake3(input, None),
        "input_manifest_sha256_pinned_and_byte_checked": dataset_manifest_sha256(dataset),
        "input_manifest_blake3_actual": blake3::hash(&std::fs::read(if dataset.name == FIXTURE_DATASET.name { fixture_manifest } else { corpus_manifest }).expect("read actual input manifest")).to_hex().to_string()
    })
}

fn new_receipt_path(timer_step_ns: u128, selection: &GroupSelection) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = root.join(".artifacts/issue384-implementation/baseline/mcp");
    std::fs::create_dir_all(&output).expect("create owned MCP receipts directory");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let path = output.join(format!("metrics-{}-{stamp}.jsonl", std::process::id()));
    let header = json!({
        "record": "header",
        "schema_version": 2,
        "transport": "direct stdio; actual bsl-analyzer-app PID; BSL_MCP_BROKER=0",
        "rss_source": "/proc/<owned-pid>/status VmRSS and VmHWM; pair sampled every 25 ms; HWM sampled before cleanup",
        "latency_unit": "microseconds from std::time::Instant",
        "timer_calibration": {
            "method": "minimum positive elapsed duration across 1024 immediate Instant measurements",
            "observed_step_ns": timer_step_ns,
            "recording_granularity_us": 1
        },
        "measurement_groups": if selection.partial { "selected groups only; 30 semantic cycles per selected measured group; this receipt does not claim full-baseline coverage" } else { "five cold and five warm per dataset; prime excluded; 30 semantic request/update/change-restore cycles per measured run; five independent 30-cycle source-scope groups" },
        "requested_group_selection": if selection.partial { selection.requested.as_str() } else { "all" },
        "selected_group_keys": selection.labels(),
        "selection_is_partial": selection.partial,
        "effective_settings": "30 disable/restore cycles in every cold/warm measured run; source-scope reconnect is a separate five-group series"
    });
    let mut file = std::fs::File::create(&path).expect("create incremental MCP receipt");
    serde_json::to_writer(&mut file, &header).expect("write MCP receipt header");
    file.write_all(b"\n").expect("terminate receipt header");
    path
}

fn append_checkpoint(path: &Path, record: Value) {
    let mut file =
        std::fs::OpenOptions::new().append(true).open(path).expect("open incremental MCP receipt");
    serde_json::to_writer(&mut file, &record).expect("append MCP receipt record");
    file.write_all(b"\n").expect("terminate MCP receipt record");
    file.sync_data().expect("persist completed MCP measurement receipt");
}

fn write_scope_modules(workspace: &Path) {
    std::fs::write(
        workspace.join("CommonModules/ScopePerfCaller.xml"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MetaDataObject xmlns=\"http://v8.1c.ru/8.3/MDClasses\"><CommonModule uuid=\"30000000-0000-0000-0000-000000000030\"><Properties><Name>ScopePerfCaller</Name><Server>true</Server><ServerCall>true</ServerCall></Properties></CommonModule></MetaDataObject>\n",
    )
    .expect("write source-scope caller XML module");
    for index in 0..METRIC_CYCLES {
        let name = format!("Scope{index:02}");
        let uuid = format!("30000000-0000-0000-0000-{index:012}");
        std::fs::write(
            workspace.join("CommonModules").join(format!("{name}.xml")),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<MetaDataObject xmlns=\"http://v8.1c.ru/8.3/MDClasses\"><CommonModule uuid=\"{uuid}\"><Properties><Name>{name}</Name><Server>true</Server><ServerCall>true</ServerCall></Properties></CommonModule></MetaDataObject>\n"
            ),
        )
        .expect("write source-scope XML module");
        let module = workspace.join("CommonModules").join(name).join("Ext");
        std::fs::create_dir_all(&module).expect("create source-scope BSL module");
        std::fs::write(
            module.join("Module.bsl"),
            format!("Функция Probe() Экспорт\n    Возврат {index};\nКонецФункции\n"),
        )
        .expect("write source-scope BSL module");
    }
}

fn scope_caller(base: &str, module: &str) -> (String, u64) {
    let mut text = base.trim_end().to_owned();
    text.push_str(&format!(
        "\n\nПроцедура ScopePerfCaller() Экспорт\n    {module}.Probe();\nКонецПроцедуры\n"
    ));
    let line = text
        .lines()
        .position(|line| line.contains(&format!("{module}.Probe")))
        .expect("scope call line") as u64;
    (text, line)
}

fn config_with_exclusions(base: &str, excluded: &[String]) -> String {
    let exclusions = excluded
        .iter()
        .map(|module| format!("\"CommonModules/{module}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let replacement = format!("root = \".\"\nexclude = [{exclusions}]\n");
    let updated = base.replacen("root = \".\"\n", &replacement, 1);
    assert_ne!(updated, base, "source config must have exactly the declared root field");
    updated
}

fn config_with_unresolved_bare_call(base: &str, enabled: bool) -> String {
    let mut replacements = 0;
    let updated = base
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("UnresolvedBareCall =") {
                replacements += 1;
                format!("UnresolvedBareCall = {enabled}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_eq!(replacements, 1, "pinned dataset must declare UnresolvedBareCall once");
    updated
}

fn source_identity_snapshot(workspace: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    for entry in walkdir::WalkDir::new(workspace) {
        let entry = entry.expect("source identity walk entry");
        if !entry.file_type().is_file() || entry.file_name() == "bsl-analyzer.toml" {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(workspace)
            .expect("source identity relative path")
            .to_path_buf();
        snapshot.insert(relative, std::fs::read(entry.path()).expect("read source identity file"));
    }
    snapshot
}

fn bsl_identity_snapshot(workspace: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    for entry in walkdir::WalkDir::new(workspace) {
        let entry = entry.expect("BSL identity walk entry");
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "bsl") {
            continue;
        }
        let relative =
            entry.path().strip_prefix(workspace).expect("BSL identity relative path").to_path_buf();
        snapshot.insert(relative, std::fs::read(entry.path()).expect("read BSL identity file"));
    }
    snapshot
}

async fn call(
    client: &Client,
    tool: &'static str,
    arguments: serde_json::Map<String, Value>,
) -> Value {
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("{tool} transport call failed: {error}"))
        .structured_content
        .unwrap_or_else(|| panic!("{tool} returned no structured content"))
}

async fn wait_diagnostic(client: &Client, dataset: Dataset) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let result = call(
            client,
            "diagnostics",
            args(&[
                ("action", json!("file")),
                ("path", json!(dataset.caller_path)),
                ("min_severity", json!("hint")),
                ("detail", json!("detailed")),
            ]),
        )
        .await;
        if result["status"] != "loading" {
            let findings = result["result"]["findings"].as_array().expect("diagnostics findings");
            assert!(
                findings.iter().any(|finding| finding["code"] == "UnresolvedBareCall"),
                "expected fixture diagnostic in resident-ready result: {result}"
            );
            assert!(
                findings.iter().any(|finding| {
                    finding["code"] == "UnresolvedBareCall"
                        && finding["message"]
                            .as_str()
                            .is_some_and(|message| message.contains(dataset.diagnostic_name))
                }),
                "expected exact {} diagnostic in resident-ready result: {result}",
                dataset.diagnostic_name
            );
            return result;
        }
        assert!(tokio::time::Instant::now() < deadline, "diagnostics never became resident-ready");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_diagnostic_absent(client: &Client, dataset: Dataset) -> Value {
    wait_diagnostic_named_absent(client, dataset, dataset.diagnostic_name).await
}

async fn wait_diagnostic_named_absent(client: &Client, dataset: Dataset, name: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let result = call(
            client,
            "diagnostics",
            args(&[
                ("action", json!("file")),
                ("path", json!(dataset.caller_path)),
                ("min_severity", json!("hint")),
                ("detail", json!("detailed")),
            ]),
        )
        .await;
        if result["status"] != "loading" {
            let findings = result["result"]["findings"]
                .as_array()
                .expect("diagnostics findings while effective setting is disabled");
            if !findings.iter().any(|finding| {
                finding["code"] == "UnresolvedBareCall"
                    && finding["message"].as_str().is_some_and(|message| message.contains(name))
            }) {
                return result;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "effective setting did not remove {} diagnostic: {result}",
            name
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_symbol_type(client: &Client, symbol: &str, expected: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let result = call(client, "symbol_info", args(&[("symbol", json!(symbol))])).await;
        if result["status"] == "ok" && result["return_type"] == expected {
            return result;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected return_type {expected}: {result}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_symbol_status(client: &Client, symbol: &str, expected: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let result = call(client, "symbol_info", args(&[("symbol", json!(symbol))])).await;
        if result["status"] == expected {
            return result;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected symbol status {expected} for {symbol}: {result}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_graph_ready(client: &Client, started: Instant) -> (u128, u128, Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let result = call(client, "graph", args(&[("action", json!("status"))])).await;
        if result["state"] == "ready" {
            let ready_us = started.elapsed().as_micros();
            let overview_started = Instant::now();
            let overview = call(client, "graph", args(&[("action", json!("overview"))])).await;
            assert!(
                overview["result"].is_object(),
                "graph overview must contain an expected result: {overview}"
            );
            return (ready_us, overview_started.elapsed().as_micros(), overview);
        }
        assert!(tokio::time::Instant::now() < deadline, "graph never became ready: {result}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_scope_retirement(mcp: &mut McpChild, changed_at: Instant) -> (u128, ExitStatus) {
    const SCOPE_STOP: &str = "workspace source composition changed; restart/reconnect MCP";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut status = None;
    loop {
        if status.is_none() {
            status = mcp.process.try_wait().expect("poll old-scope MCP process");
        }
        let stderr = mcp.stderr.lock().expect("MCP stderr lock").clone();
        if let Some(exit) = status.as_ref() {
            if stderr.contains(SCOPE_STOP) {
                assert!(exit.success(), "old-scope MCP retirement exits successfully: {exit}");
                assert_eq!(
                    stderr.matches(SCOPE_STOP).count(),
                    1,
                    "one source-scope retirement reason"
                );
                return (
                    changed_at.elapsed().as_micros(),
                    status.take().expect("retired child status"),
                );
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "old MCP did not retire with scope reason; stderr={stderr}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn scope_config_update_series(
    workspace: &Path,
    dataset: Dataset,
    group: usize,
    receipt_path: &Path,
    input_hashes: &Value,
) -> (Vec<ScopeRun>, Vec<LatencySample>) {
    write_scope_modules(workspace);
    let caller_base = std::fs::read_to_string(workspace.join(dataset.caller_path))
        .expect("read source-scope caller fixture");
    let (initial_caller, _) = scope_caller(&caller_base, "Scope00");
    let scope_caller_path = workspace.join(SCOPE_CALLER);
    std::fs::create_dir_all(scope_caller_path.parent().expect("scope caller parent"))
        .expect("create source-scope caller folder");
    std::fs::write(&scope_caller_path, &initial_caller).expect("write scope caller");
    let config_path = workspace.join("bsl-analyzer.toml");
    let base_config = std::fs::read_to_string(&config_path).expect("read source config");
    let cache = tempfile::tempdir().expect("owned source-scope cache");
    let mut lsp = OwnedLsp::start(workspace, cache.path(), SCOPE_CALLER, &initial_caller);
    let lsp_pid = lsp.child.id();
    let mut excluded = Vec::with_capacity(METRIC_CYCLES);
    let mut runs = Vec::with_capacity(METRIC_CYCLES);
    let mut samples = Vec::with_capacity(METRIC_CYCLES * 3);

    for cycle in 0..METRIC_CYCLES {
        let name = format!("Scope{cycle:02}");
        let symbol = format!("{name}.Probe");
        let (caller, call_line) = scope_caller(&caller_base, &name);
        std::fs::write(&scope_caller_path, &caller).expect("refresh saved scope caller");
        lsp.set_caller_text(workspace, SCOPE_CALLER, &caller);
        let old_lsp_poll = lsp.wait_definition_path_at(SCOPE_CALLER, workspace, call_line, &name);

        let old_started = Instant::now();
        let mut old = McpChild::start(workspace, cache.path()).await;
        let old_pid = old.pid;
        let old_sampler = PairSampler::start(old_pid, lsp_pid);
        let _diagnostic = wait_diagnostic(&old.client, dataset).await;
        let _retained = wait_symbol_type(&old.client, dataset.symbol, "Число").await;
        let old_scope = wait_symbol_status(&old.client, &symbol, "ok").await;
        assert_eq!(old_scope["status"], "ok", "old composition includes {symbol}: {old_scope}");
        if let Some(global_symbol) = dataset.global_symbol {
            let global = wait_symbol_status(&old.client, global_symbol, "ok").await;
            assert_eq!(global["status"], "ok", "old composition includes {global_symbol}");
        }
        let _old_ready_us = old_started.elapsed().as_micros();

        excluded.push(name.clone());
        let new_config = config_with_exclusions(&base_config, &excluded);
        let changed_at = Instant::now();
        std::fs::write(&config_path, new_config).expect("apply source-scope config change");
        lsp.watched_config(workspace);
        let lsp_future = tokio::task::spawn_blocking({
            let mut lsp = lsp;
            let root = workspace.to_path_buf();
            move || {
                let poll = lsp.wait_definition_absent_at(SCOPE_CALLER, &root, call_line);
                (lsp, poll)
            }
        });
        let (retired_us, _old_exit) = wait_scope_retirement(&mut old, changed_at).await;
        let (updated_lsp, lsp_new_poll) = lsp_future.await.expect("LSP config-refresh task");
        lsp = updated_lsp;
        let lsp_new_result_us = lsp_new_poll.elapsed_us;
        let old_pair_rss = old_sampler.finish();
        let old_mcp_stderr = old.stderr.lock().expect("old MCP stderr").clone();
        old.stop().await;

        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group: "scope-reconnect",
            run: group,
            kind: "lsp-before-config-old-composition",
            iteration: cycle,
            expected: format!(
                "textDocument/definition({symbol}) resolves old composition; attempts={}, ContentModified interruptions={}",
                old_lsp_poll.attempts, old_lsp_poll.content_modified_interruptions
            ),
            elapsed_us: old_lsp_poll.elapsed_us,
            lsp_client_poll_attempts: Some(old_lsp_poll.attempts),
            lsp_content_modified_interruptions: Some(
                old_lsp_poll.content_modified_interruptions,
            ),
        });
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group: "scope-reconnect",
            run: group,
            kind: "scope-config-to-mcp-retired",
            iteration: cycle,
            expected: "old stdio process exits successfully with source-composition-changed reason"
                .to_owned(),
            elapsed_us: retired_us,
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group: "scope-reconnect",
            run: group,
            kind: "scope-config-to-lsp-new-composition",
            iteration: cycle,
            expected: format!(
                "textDocument/definition({symbol}) returns no location; attempts={}, ContentModified interruptions={}",
                lsp_new_poll.attempts, lsp_new_poll.content_modified_interruptions
            ),
            elapsed_us: lsp_new_result_us,
            lsp_client_poll_attempts: Some(lsp_new_poll.attempts),
            lsp_content_modified_interruptions: Some(
                lsp_new_poll.content_modified_interruptions,
            ),
        });

        let reconnected = McpChild::start(workspace, cache.path()).await;
        let new_pid = reconnected.pid;
        let reconnected_sampler = PairSampler::start(new_pid, lsp_pid);
        let _diagnostic = wait_diagnostic(&reconnected.client, dataset).await;
        let retained = wait_symbol_type(&reconnected.client, dataset.symbol, "Число").await;
        let excluded_result = wait_symbol_status(&reconnected.client, &symbol, "not_found").await;
        assert_eq!(retained["status"], "ok", "new scope retained {}: {retained}", dataset.symbol);
        assert_eq!(excluded_result["status"], "not_found", "new scope excludes {symbol}");
        let global_status = if let Some(global_symbol) = dataset.global_symbol {
            let global = wait_symbol_status(&reconnected.client, global_symbol, "ok").await;
            Some(global["status"].as_str().unwrap().to_owned())
        } else {
            None
        };
        let reconnected_us = changed_at.elapsed().as_micros();
        let reconnected_pair_rss = reconnected_sampler.finish();
        let reconnected_mcp_stderr =
            reconnected.stderr.lock().expect("reconnected MCP stderr").clone();
        reconnected.stop().await;

        let mut expected_composition = format!("{}=ok; ", dataset.symbol);
        if let Some(global_symbol) = dataset.global_symbol {
            expected_composition.push_str(&format!("{global_symbol}=ok; "));
        }
        expected_composition.push_str(&format!("{symbol}=not_found"));
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group: "scope-reconnect",
            run: group,
            kind: "scope-config-to-mcp-reconnected-composition",
            iteration: cycle,
            expected: expected_composition,
            elapsed_us: reconnected_us,
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
        let scope_run = ScopeRun {
            dataset: dataset.name,
            group,
            cycle,
            excluded_module: name,
            old_mcp_pid: old_pid,
            new_mcp_pid: new_pid,
            lsp_pid,
            old_scope_symbol: symbol,
            new_scope_symbol_status: excluded_result["status"].as_str().unwrap().to_owned(),
            retained_symbol_status: retained["status"].as_str().unwrap().to_owned(),
            global_symbol_status: global_status,
            lsp_old_composition_poll_attempts: old_lsp_poll.attempts,
            lsp_old_composition_content_modified_interruptions: old_lsp_poll
                .content_modified_interruptions,
            lsp_new_composition_poll_attempts: lsp_new_poll.attempts,
            lsp_new_composition_content_modified_interruptions: lsp_new_poll
                .content_modified_interruptions,
            config_to_old_mcp_retired_us: retired_us,
            config_to_lsp_new_composition_us: lsp_new_result_us,
            config_to_mcp_reconnected_us: reconnected_us,
            old_mcp_stderr,
            reconnected_mcp_stderr,
            old_pair_rss,
            reconnected_pair_rss,
        };
        append_checkpoint(
            receipt_path,
            json!({
                "record": "source-scope-cycle",
                "dataset": dataset.name,
                "group": group,
                "hashes": input_hashes,
                "scope_run": &scope_run,
                "semantic_latency_samples": &samples[samples.len() - 3..]
            }),
        );
        runs.push(scope_run);
    }
    lsp.shutdown();
    (runs, samples)
}

fn assert_mcp_stayed_in_same_source_scope(mcp: &mut McpChild, expected_pid: u32) {
    assert_eq!(mcp.pid, expected_pid, "effective settings keep the same MCP process");
    let status = mcp.process.try_wait().expect("poll MCP after effective settings reload");
    assert!(
        status.is_none(),
        "effective setting change must not retire MCP source scope; exit={status:?}, stderr={}",
        mcp.stderr.lock().expect("MCP stderr").as_str()
    );
    assert!(
        !mcp.stderr
            .lock()
            .expect("MCP stderr")
            .contains("workspace source composition changed; restart/reconnect MCP"),
        "effective setting change must not signal source-composition retirement"
    );
}

async fn retained_symbol_path(client: &Client, dataset: Dataset) -> String {
    let result = wait_symbol_type(client, dataset.symbol, "Число").await;
    let definitions = result["definitions"].as_array().expect("symbol definitions");
    let path = definitions
        .iter()
        .filter_map(|definition| definition["location"]["path"].as_str())
        .find(|path| path.ends_with(dataset.target_path))
        .unwrap_or_else(|| panic!("symbol identity moved from {}: {result}", dataset.target_path));
    path.to_owned()
}

async fn effective_settings_update_series(
    mcp: &mut McpChild,
    lsp_pid: u32,
    workspace: &Path,
    dataset: Dataset,
    cache_group: &'static str,
    run: usize,
) -> (Vec<EffectiveSettingsRun>, Vec<LatencySample>) {
    let config_path = workspace.join("bsl-analyzer.toml");
    let original_config = std::fs::read_to_string(&config_path).expect("read effective config");
    let enabled_config = config_with_unresolved_bare_call(&original_config, true);
    assert_eq!(
        original_config, enabled_config,
        "pinned measurement inputs begin with the known diagnostic enabled"
    );
    let disabled_config = config_with_unresolved_bare_call(&original_config, false);
    let source_identity = source_identity_snapshot(workspace);
    let retained_path = retained_symbol_path(&mcp.client, dataset).await;
    let mcp_pid = mcp.pid;
    let mut runs = Vec::with_capacity(METRIC_CYCLES);
    let mut samples = Vec::with_capacity(METRIC_CYCLES * 2);

    for cycle in 0..METRIC_CYCLES {
        let before = wait_diagnostic_present(&mcp.client, dataset).await;
        assert_mcp_stayed_in_same_source_scope(mcp, mcp_pid);
        assert!(has_known_diagnostic(&before, dataset));

        let disabled_started = Instant::now();
        std::fs::write(&config_path, &disabled_config).expect("disable known diagnostic setting");
        let _disabled = wait_diagnostic_absent(&mcp.client, dataset).await;
        let disabled_us = disabled_started.elapsed().as_micros();
        assert_mcp_stayed_in_same_source_scope(mcp, mcp_pid);
        let disabled_symbol_path = retained_symbol_path(&mcp.client, dataset).await;
        assert_eq!(disabled_symbol_path, retained_path, "symbol identity while rule disabled");
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "effective-settings-to-diagnostic-absent",
            iteration: cycle,
            expected: format!(
                "{} absent; {} remains at {}",
                dataset.diagnostic_name, dataset.symbol, retained_path
            ),
            elapsed_us: disabled_us,
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });

        let restored_started = Instant::now();
        std::fs::write(&config_path, &enabled_config).expect("restore known diagnostic setting");
        let restored = wait_diagnostic_present(&mcp.client, dataset).await;
        let restored_us = restored_started.elapsed().as_micros();
        assert_mcp_stayed_in_same_source_scope(mcp, mcp_pid);
        let restored_symbol_path = retained_symbol_path(&mcp.client, dataset).await;
        assert_eq!(restored_symbol_path, retained_path, "symbol identity after rule restore");
        assert!(has_known_diagnostic(&restored, dataset));
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "effective-settings-to-diagnostic-restored-present",
            iteration: cycle,
            expected: format!(
                "{} present; source scope remains on PID {mcp_pid}",
                dataset.diagnostic_name
            ),
            elapsed_us: restored_us,
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
        runs.push(EffectiveSettingsRun {
            dataset: dataset.name,
            dataset_manifest_sha256: dataset_manifest_sha256(dataset),
            cache_group,
            run,
            cycle,
            mcp_pid,
            lsp_pid,
            diagnostic_code: "UnresolvedBareCall",
            diagnostic_name: dataset.diagnostic_name,
            retained_symbol: dataset.symbol,
            retained_symbol_path: retained_path.clone(),
            source_identity_unchanged: false,
            config_to_disabled_absent_us: disabled_us,
            config_to_restored_present_us: restored_us,
        });
    }

    assert!(
        source_identity_snapshot(workspace) == source_identity,
        "effective settings updates must not modify any source input"
    );
    for run in &mut runs {
        run.source_identity_unchanged = true;
    }
    (runs, samples)
}

async fn measured_launch(
    workspace: &Path,
    cache: &Path,
    dataset: Dataset,
    cache_kind: &'static str,
    run: usize,
) -> (ProcessRun, McpChild, OwnedLsp, PairSampler) {
    let mcp_started = Instant::now();
    let mcp = McpChild::start(workspace, cache).await;
    let handshake_us = mcp_started.elapsed().as_micros();
    let handshake_at = Instant::now();
    let controls_started = Instant::now();
    let catalog = call(&mcp.client, "diagnostics", args(&[("action", json!("catalog"))])).await;
    let diagnostics_schema =
        call(&mcp.client, "diagnostics", args(&[("action", json!("schema"))])).await;
    let graph_schema = call(&mcp.client, "graph", args(&[("action", json!("schema"))])).await;
    let platform = call(&mcp.client, "syntax_help", args(&[("name", json!("Массив"))])).await;
    assert!(catalog["entries"].is_array(), "cold diagnostics catalog control: {catalog}");
    assert!(
        diagnostics_schema["actions"].is_array(),
        "diagnostics cold schema control: {diagnostics_schema}"
    );
    assert!(graph_schema["actions"].is_array(), "graph cold schema control: {graph_schema}");
    assert_eq!(platform["schema_version"], "2", "cold platform-card contract: {platform}");
    assert_eq!(platform["kind"], "type", "syntax_help resolves an actual type card: {platform}");
    assert_eq!(platform["name"], "Массив", "syntax_help returns the requested type: {platform}");
    assert!(platform["constructors"].is_array(), "type card constructors: {platform}");
    assert!(platform["methods"].is_array(), "type card methods: {platform}");
    let controls_us = controls_started.elapsed().as_micros();

    // The source handlers keep schema/catalog static. These are the first explicit
    // lifecycle controls and are timed separately because each may start its build.
    let diagnostics_status_started = Instant::now();
    let diagnostics_status =
        call(&mcp.client, "diagnostics", args(&[("action", json!("status"))])).await;
    let diagnostics_status_us = diagnostics_status_started.elapsed().as_micros();
    assert!(
        diagnostics_status["state"].is_string() || diagnostics_status["status"].is_string(),
        "explicit diagnostics status control must report lifecycle state: {diagnostics_status}"
    );
    let graph_status_started = Instant::now();
    let graph_status = call(&mcp.client, "graph", args(&[("action", json!("status"))])).await;
    let graph_status_us = graph_status_started.elapsed().as_micros();
    assert!(graph_status["state"].is_string(), "explicit graph status control: {graph_status}");

    let resident_future = async {
        let _diagnostic = wait_diagnostic(&mcp.client, dataset).await;
        let _symbol = wait_symbol_type(&mcp.client, dataset.symbol, "Число").await;
        if let Some(global_symbol) = dataset.global_symbol {
            let _global = wait_symbol_type(&mcp.client, global_symbol, "Число").await;
        }
        handshake_at.elapsed().as_micros()
    };
    let lsp_future = tokio::task::spawn_blocking({
        let root = workspace.to_path_buf();
        let cache = cache.to_path_buf();
        let caller_path = dataset.caller_path;
        let mcp_pid = mcp.pid;
        move || {
            let started = Instant::now();
            let caller_text =
                std::fs::read_to_string(root.join(caller_path)).expect("read LSP caller");
            let (lsp, sampler) = OwnedLsp::start_with_pair_sampler(
                &root,
                &cache,
                caller_path,
                &caller_text,
                mcp_pid,
            );
            (lsp, started.elapsed().as_micros(), sampler)
        }
    });
    let graph_future = wait_graph_ready(&mcp.client, graph_status_started);
    let (resident_us, lsp_result, (graph_ready_us, graph_overview_us, _)) =
        tokio::join!(resident_future, lsp_future, graph_future);
    let (lsp, lsp_us, sampler) = lsp_result.expect("LSP peer start task");
    let lsp_pid = lsp.child.id();
    (
        ProcessRun {
            dataset: dataset.name,
            dataset_manifest_sha256: Some(dataset_manifest_sha256(dataset)),
            cache: cache_kind,
            run,
            mcp_pid: mcp.pid,
            lsp_pid,
            lazy_control_requests_us: controls_us,
            process_to_handshake_us: handshake_us,
            explicit_diagnostics_status_us: diagnostics_status_us,
            explicit_graph_status_us: graph_status_us,
            diagnostics_status_after_static_controls: diagnostics_status,
            graph_status_after_static_controls: graph_status,
            handshake_to_resident_result_us: resident_us,
            handshake_to_graph_ready_us: graph_ready_us,
            graph_overview_us,
            lsp_initialize_and_open_us: lsp_us,
            mcp_hwm_kib_before_cleanup: 0,
            lsp_hwm_kib_before_cleanup: 0,
            rss: Vec::new(),
        },
        mcp,
        lsp,
        sampler,
    )
}

async fn request_samples(
    client: &Client,
    dataset: Dataset,
    cache_group: &'static str,
    run: usize,
) -> Vec<LatencySample> {
    let group_count = 2 + usize::from(dataset.global_symbol.is_some());
    let mut samples = Vec::with_capacity(METRIC_CYCLES * group_count);
    for iteration in 0..METRIC_CYCLES * group_count {
        let kind = match iteration % group_count {
            0 => "diagnostics",
            1 => "symbol_info",
            _ => "global_symbol_info",
        };
        let (tool, arguments, expected) = match kind {
            "diagnostics" => (
                "diagnostics",
                args(&[
                    ("action", json!("file")),
                    ("path", json!(dataset.caller_path)),
                    ("min_severity", json!("hint")),
                    ("detail", json!("detailed")),
                ]),
                "UnresolvedBareCall".to_owned(),
            ),
            "symbol_info" => (
                "symbol_info",
                args(&[("symbol", json!(dataset.symbol))]),
                "return_type is Число".to_owned(),
            ),
            "global_symbol_info" => (
                "symbol_info",
                args(&[("symbol", json!(dataset.global_symbol.expect("global symbol")))]),
                "global return_type is Число".to_owned(),
            ),
            _ => unreachable!("defined request group"),
        };
        let started = Instant::now();
        let result = if kind == "diagnostics" {
            wait_diagnostic_present(client, dataset).await
        } else {
            call(client, tool, arguments).await
        };
        if kind == "diagnostics" {
            assert!(
                has_known_diagnostic(&result, dataset),
                "request sample missed known {} diagnostic: {result}",
                dataset.diagnostic_name
            );
        } else {
            assert_eq!(result["status"], "ok", "request sample missed known symbol: {result}");
            assert_eq!(result["return_type"], "Число", "symbol result: {result}");
        }
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind,
            iteration,
            expected,
            elapsed_us: started.elapsed().as_micros(),
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
    }
    samples
}

async fn semantic_update_samples(
    client: &Client,
    workspace: &Path,
    dataset: Dataset,
    cache_group: &'static str,
    run: usize,
) -> Vec<LatencySample> {
    let target = workspace.join(dataset.target_path);
    let original_target = std::fs::read(&target).expect("read baseline BSL semantic source");
    let mut samples = Vec::with_capacity(METRIC_CYCLES * 4);
    for iteration in 0..METRIC_CYCLES {
        let source = format!(
            "Функция {}() Экспорт\n    Возврат \"metric-update-{iteration}\";\nКонецФункции\n",
            dataset.target_function
        );
        let changed_started = Instant::now();
        std::fs::write(&target, source).expect("write semantic BSL update");
        let result = wait_symbol_type(client, dataset.symbol, "Строка").await;
        assert_eq!(result["status"], "ok", "BSL change result: {result}");
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "bsl-update-to-string-result",
            iteration,
            expected: "symbol return_type is Строка".to_owned(),
            elapsed_us: changed_started.elapsed().as_micros(),
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
        let restored_started = Instant::now();
        std::fs::write(&target, &original_target).expect("restore semantic BSL source");
        let restored = wait_symbol_type(client, dataset.symbol, "Число").await;
        assert_eq!(restored["status"], "ok", "BSL restore result: {restored}");
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "bsl-restore-to-number-result",
            iteration,
            expected: "symbol return_type is Число".to_owned(),
            elapsed_us: restored_started.elapsed().as_micros(),
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
    }

    let xml_path = workspace.join(dataset.global_xml_path);
    let original_xml = std::fs::read(&xml_path).expect("read pinned Global=true descriptor");
    let original_xml_text = std::str::from_utf8(&original_xml).expect("XML descriptor is UTF-8");
    let bsl_snapshot = bsl_identity_snapshot(workspace);
    let baseline = wait_diagnostic_present(client, dataset).await;
    assert!(has_known_diagnostic(&baseline, dataset));
    assert!(
        !has_unresolved_bare_call(&baseline, dataset.global_bare_call),
        "{} resolves while Global is true: {baseline}",
        dataset.global_bare_call
    );
    let xml_with_global_false = xml_global_setting(original_xml_text, false);
    assert_ne!(xml_with_global_false.as_bytes(), original_xml.as_slice());
    for iteration in 0..METRIC_CYCLES {
        let started = Instant::now();
        std::fs::write(&xml_path, &xml_with_global_false).expect("disable Global on pinned module");
        let unresolved =
            wait_diagnostic_named_present(client, dataset, dataset.global_bare_call).await;
        assert!(has_unresolved_bare_call(&unresolved, dataset.global_bare_call));
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "xml-global-false-to-bare-call-diagnostic-present",
            iteration,
            expected: format!(
                "UnresolvedBareCall({}) present after XML Global=false",
                dataset.global_bare_call
            ),
            elapsed_us: started.elapsed().as_micros(),
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });

        let restored_started = Instant::now();
        std::fs::write(&xml_path, &original_xml).expect("restore pinned Global=true descriptor");
        let restored =
            wait_diagnostic_named_absent(client, dataset, dataset.global_bare_call).await;
        assert!(has_known_diagnostic(&restored, dataset));
        samples.push(LatencySample {
            dataset: dataset.name,
            cache_group,
            run,
            kind: "xml-global-restore-to-bare-call-diagnostic-absent",
            iteration,
            expected: format!(
                "UnresolvedBareCall({}) absent after restoring XML Global=true",
                dataset.global_bare_call
            ),
            elapsed_us: restored_started.elapsed().as_micros(),
            lsp_client_poll_attempts: None,
            lsp_content_modified_interruptions: None,
        });
    }
    assert_eq!(
        bsl_identity_snapshot(workspace),
        bsl_snapshot,
        "XML Global toggles must leave every BSL source byte unchanged"
    );
    assert_eq!(
        std::fs::read(&xml_path).expect("read restored pinned XML descriptor"),
        original_xml,
        "each XML cycle must restore the exact pinned Global=true bytes"
    );
    samples
}

fn xml_global_setting(xml: &str, enabled: bool) -> String {
    assert_eq!(xml.matches("<Global>true</Global>").count(), 1, "pinned Global=true XML");
    xml.replace(
        "<Global>true</Global>",
        if enabled { "<Global>true</Global>" } else { "<Global>false</Global>" },
    )
}

fn instant_resolution_ns() -> u128 {
    (0..1024)
        .map(|_| {
            let started = Instant::now();
            loop {
                let elapsed = started.elapsed().as_nanos();
                if elapsed > 0 {
                    break elapsed;
                }
            }
        })
        .min()
        .expect("Instant calibration samples")
}

fn write_receipt(
    runs: &[ProcessRun],
    scope_runs: &[ScopeRun],
    effective_settings_runs: &[EffectiveSettingsRun],
    samples: &[LatencySample],
    measured_instant_step_ns: u128,
    selection: &GroupSelection,
) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = root.join(".artifacts/issue384-implementation/baseline/mcp");
    std::fs::create_dir_all(&output).expect("create owned MCP receipts directory");
    let receipt = json!({
        "schema_version": 1,
        "transport": "direct stdio; actual bsl-analyzer-app PID; no broker proxy",
        "rss_source": "/proc/<owned-pid>/status VmRSS and VmHWM; pair sampled every 25 ms; sequential read skew recorded per sample",
        "latency_unit": "microseconds from std::time::Instant; pair RSS values are near-simultaneous process samples with recorded read skew, not summed HWM",
        "timer_calibration": {
            "method": "minimum positive elapsed duration across 1024 immediate Instant measurements",
            "observed_step_ns": measured_instant_step_ns,
            "recording_granularity_us": 1
        },
        "measurement_groups": if selection.partial { "selected groups only; 30 semantic cycles per selected measured group; this receipt does not claim full-baseline coverage" } else { "five cold and five warm per dataset; prime excluded; 30 semantic request/update/change-restore cycles per measured run; five independent 30-cycle source-scope groups" },
        "requested_group_selection": if selection.partial { selection.requested.as_str() } else { "all" },
        "selected_group_keys": selection.labels(),
        "selection_is_partial": selection.partial,
        "corpus_1000_v4": {
            "manifest_path": ".artifacts/issue384-implementation/baseline/corpus-1000-v4/input-manifest.json",
            "workspace_sha256": CORPUS_MANIFEST_HASH,
            "initial_effective_settings": "copied bsl-analyzer.toml from pinned v4 manifest; UnresolvedBareCall=true before measurement"
        },
        "perf_shadow_fixture": {
            "manifest_path": ".artifacts/issue384-implementation/baseline/shared-perf-shadow-manifest.json",
            "workspace_sha256": PERF_SHADOW_MANIFEST_HASH,
            "effective_settings": "diagnostics.parameters.UnresolvedBareCall=true"
        },
        "effective_settings_toggle": if selection.partial { "30 expected absent/present disable-restore cycles for each selected cold/warm run; scope retirement/reconnect is a separate selected series" } else { "30 expected absent/present disable-restore cycles for every one of five cold and five warm runs per dataset; source-scope retirement/reconnect has separate five-group series" },
        "runs": runs,
        "scope_runs": scope_runs,
        "effective_settings_runs": effective_settings_runs,
        "semantic_latency_samples": samples,
    });
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let path = output.join(format!("metrics-{}-{stamp}.json", std::process::id()));
    std::fs::write(path, serde_json::to_vec_pretty(&receipt).expect("serialize receipt"))
        .expect("write MCP metrics receipt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long baseline/candidate measurement; run only under the coordinated release harness"]
async fn records_external_mcp_and_paired_lsp_workspace_metrics() {
    require_release_profile(cfg!(debug_assertions));
    let measured_instant_step_ns = instant_resolution_ns();
    let selection = GroupSelection::from_env();
    let receipt_path = new_receipt_path(measured_instant_step_ns, &selection);
    let mut runs = Vec::new();
    let mut scope_runs = Vec::new();
    let mut effective_settings_runs = Vec::new();
    let mut latency_samples = Vec::new();
    for dataset in [FIXTURE_DATASET, CORPUS_DATASET] {
        let source = if dataset.name == FIXTURE_DATASET.name {
            perf_shadow_fixture()
        } else {
            corpus_1000_v4()
        };
        let input_hashes = actual_hashes(dataset, source.path());

        for cache_group in ["cold", "warm"] {
            if !(1..=RUNS_PER_GROUP)
                .any(|run_index| selection.contains(dataset.name, cache_group, run_index))
            {
                continue;
            }
            // Keep one workspace path per cache group so the warm cache is primed
            // and measured against the same source identity and path.
            let group_workspace = clone_workspace(source.path());
            let warm_cache = tempfile::tempdir().expect("owned warm cache");
            if cache_group == "warm"
                && (1..=RUNS_PER_GROUP)
                    .any(|run_index| selection.contains(dataset.name, cache_group, run_index))
            {
                let (_prime, mcp, mut lsp, sampler) =
                    measured_launch(group_workspace.path(), warm_cache.path(), dataset, "prime", 0)
                        .await;
                let _prime_rss = sampler.finish();
                mcp.stop().await;
                lsp.shutdown();
                reset_workspace(group_workspace.path(), source.path());
            }

            for run_index in 0..RUNS_PER_GROUP {
                if !selection.contains(dataset.name, cache_group, run_index + 1) {
                    continue;
                }
                reset_workspace(group_workspace.path(), source.path());
                let cold_cache;
                let cache = if cache_group == "cold" {
                    cold_cache = tempfile::tempdir().expect("owned cold cache");
                    cold_cache.path()
                } else {
                    warm_cache.path()
                };
                let (mut run, mut mcp, mut lsp, sampler) = measured_launch(
                    group_workspace.path(),
                    cache,
                    dataset,
                    cache_group,
                    run_index + 1,
                )
                .await;
                let mut run_samples =
                    request_samples(&mcp.client, dataset, cache_group, run_index + 1).await;
                let (setting_runs, setting_samples) = effective_settings_update_series(
                    &mut mcp,
                    lsp.child.id(),
                    group_workspace.path(),
                    dataset,
                    cache_group,
                    run_index + 1,
                )
                .await;
                run_samples.extend(setting_samples);
                run_samples.extend(
                    semantic_update_samples(
                        &mcp.client,
                        group_workspace.path(),
                        dataset,
                        cache_group,
                        run_index + 1,
                    )
                    .await,
                );
                run.rss = sampler.finish();
                if let Some(last) = run.rss.last() {
                    run.mcp_hwm_kib_before_cleanup = last.mcp_hwm_kib;
                    run.lsp_hwm_kib_before_cleanup = last.lsp_hwm_kib;
                }
                append_checkpoint(
                    &receipt_path,
                    json!({
                        "record": "measured-run",
                        "hashes": input_hashes,
                        "timer_step_ns": measured_instant_step_ns,
                        "run": &run,
                        "effective_settings_runs": &setting_runs,
                        "semantic_latency_samples": &run_samples
                    }),
                );
                runs.push(run);
                effective_settings_runs.extend(setting_runs);
                latency_samples.extend(run_samples);
                mcp.stop().await;
                lsp.shutdown();
            }
        }

        for group in 1..=RUNS_PER_GROUP {
            if !selection.contains(dataset.name, "scope", group) {
                continue;
            }
            let scope_workspace = clone_workspace(source.path());
            let (group_runs, group_samples) = scope_config_update_series(
                scope_workspace.path(),
                dataset,
                group,
                &receipt_path,
                &input_hashes,
            )
            .await;
            append_checkpoint(
                &receipt_path,
                json!({
                    "record": "source-scope-group",
                    "dataset": dataset.name,
                    "group": group,
                    "hashes": input_hashes,
                    "scope_runs": &group_runs,
                    "semantic_latency_samples": &group_samples
                }),
            );
            scope_runs.extend(group_runs);
            latency_samples.extend(group_samples);
        }
    }
    let cold_groups = selection.count("cold", None);
    let warm_groups = selection.count("warm", None);
    let process_groups = cold_groups + warm_groups;
    let corpus_process_groups = selection.count("cold", Some(CORPUS_DATASET.name))
        + selection.count("warm", Some(CORPUS_DATASET.name));
    let scope_groups = selection.count("scope", None);
    assert_eq!(runs.iter().filter(|run| run.cache == "cold").count(), cold_groups);
    assert_eq!(runs.iter().filter(|run| run.cache == "warm").count(), warm_groups);
    assert_eq!(runs.len(), process_groups, "each selected process group ran once");
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind == "diagnostics").count(),
        METRIC_CYCLES * process_groups,
        "each selected process group needs 30 expected-result diagnostics samples"
    );
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind == "symbol_info").count(),
        METRIC_CYCLES * process_groups,
        "each selected process group needs 30 expected-result symbol_info samples"
    );
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind == "global_symbol_info").count(),
        METRIC_CYCLES * corpus_process_groups,
        "each selected corpus process group needs 30 expected-result global symbol_info samples"
    );
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind.starts_with("bsl-")).count(),
        METRIC_CYCLES * 2 * process_groups,
        "each selected cold/warm run needs 30 BSL change/restore semantic results"
    );
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind.starts_with("xml-")).count(),
        METRIC_CYCLES * 2 * process_groups,
        "each selected cold/warm run needs 30 XML change/restore semantic results"
    );
    assert_eq!(
        latency_samples.iter().filter(|sample| sample.kind.starts_with("scope-config-")).count(),
        METRIC_CYCLES * 3 * scope_groups,
        "each selected source-scope group needs 30 retirement/LSP/reconnect cycles"
    );
    assert_eq!(scope_runs.len(), METRIC_CYCLES * scope_groups);
    assert_eq!(
        latency_samples
            .iter()
            .filter(|sample| sample.kind == "effective-settings-to-diagnostic-absent")
            .count(),
        METRIC_CYCLES * process_groups,
        "30 diagnostics-disable expected-result updates in every selected measured run"
    );
    assert_eq!(
        latency_samples
            .iter()
            .filter(|sample| sample.kind == "effective-settings-to-diagnostic-restored-present")
            .count(),
        METRIC_CYCLES * process_groups,
        "30 diagnostics-restore expected-result updates in every selected measured run"
    );
    assert_eq!(effective_settings_runs.len(), METRIC_CYCLES * process_groups);
    for dataset in [FIXTURE_DATASET, CORPUS_DATASET] {
        for cache_group in ["cold", "warm"] {
            for run in 1..=RUNS_PER_GROUP {
                if !selection.contains(dataset.name, cache_group, run) {
                    continue;
                }
                let matching = |kind: &str| {
                    latency_samples
                        .iter()
                        .filter(|sample| {
                            sample.dataset == dataset.name
                                && sample.cache_group == cache_group
                                && sample.run == run
                                && sample.kind == kind
                        })
                        .count()
                };
                for kind in ["diagnostics", "symbol_info"] {
                    assert_eq!(
                        matching(kind),
                        METRIC_CYCLES,
                        "{}/{cache_group}/{run}/{kind}",
                        dataset.name
                    );
                }
                if dataset.global_symbol.is_some() {
                    assert_eq!(matching("global_symbol_info"), METRIC_CYCLES);
                }
                for kind in [
                    "effective-settings-to-diagnostic-absent",
                    "effective-settings-to-diagnostic-restored-present",
                    "bsl-update-to-string-result",
                    "bsl-restore-to-number-result",
                    "xml-global-false-to-bare-call-diagnostic-present",
                    "xml-global-restore-to-bare-call-diagnostic-absent",
                ] {
                    assert_eq!(
                        matching(kind),
                        METRIC_CYCLES,
                        "{}/{cache_group}/{run}/{kind}",
                        dataset.name
                    );
                }
            }
        }
        for group in 1..=RUNS_PER_GROUP {
            if !selection.contains(dataset.name, "scope", group) {
                continue;
            }
            assert_eq!(
                scope_runs
                    .iter()
                    .filter(|scope| scope.dataset == dataset.name && scope.group == group)
                    .count(),
                METRIC_CYCLES,
                "{} source-scope group {group} must contain 30 cycles",
                dataset.name
            );
            assert_eq!(
                latency_samples
                    .iter()
                    .filter(|sample| {
                        sample.dataset == dataset.name
                            && sample.cache_group == "scope-reconnect"
                            && sample.run == group
                            && sample.kind == "lsp-before-config-old-composition"
                    })
                    .count(),
                METRIC_CYCLES,
                "{} source-scope group {group} records 30 old-composition LSP poll samples",
                dataset.name
            );
            for kind in [
                "scope-config-to-mcp-retired",
                "scope-config-to-lsp-new-composition",
                "scope-config-to-mcp-reconnected-composition",
            ] {
                assert_eq!(
                    latency_samples
                        .iter()
                        .filter(|sample| {
                            sample.dataset == dataset.name
                                && sample.cache_group == "scope-reconnect"
                                && sample.run == group
                                && sample.kind == kind
                        })
                        .count(),
                    METRIC_CYCLES,
                    "{} source-scope group {group}/{kind}",
                    dataset.name
                );
            }
        }
    }
    write_receipt(
        &runs,
        &scope_runs,
        &effective_settings_runs,
        &latency_samples,
        measured_instant_step_ns,
        &selection,
    );
}
