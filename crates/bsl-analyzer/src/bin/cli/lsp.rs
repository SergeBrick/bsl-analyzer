use std::{
    collections::{HashMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use lsp_server::{Connection, Message, RequestId};
use mcp_server::broker::transport::{AdapterKind, LspChannel, LspControl};

use super::mcp::shared::{self, LspContext};

#[derive(Clone, Copy, Default, ValueEnum)]
pub enum LspMode {
    #[default]
    Auto,
    Stdio,
}

#[derive(Default, Args)]
pub struct LspArgs {
    #[arg(long, value_enum, default_value = "auto")]
    mode: LspMode,
}

pub fn run_lsp_server(args: LspArgs) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (connection, io_threads) = Connection::stdio();
    let result = match args.mode {
        LspMode::Stdio => bsl_analyzer::main_loop(connection),
        LspMode::Auto => run_auto(connection),
    };
    if result.is_ok() {
        io_threads.join().ok();
    }
    result.map_err(Into::into)
}

fn run_auto(connection: Connection) -> Result<()> {
    let initial = connection.receiver.recv().context("LSP closed before initialize")?;
    let params = match &initial {
        Message::Request(request) if request.method == "initialize" => request.params.clone(),
        _ => anyhow::bail!("expected LSP initialize"),
    };
    let parsed: lsp_types::InitializeParams = serde_json::from_value(params.clone())?;
    let root = bsl_analyzer::server::extract_workspace_root(&parsed);
    let Some(root) = root else {
        return standalone_with_initial(connection, initial);
    };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let backend = match rt.block_on(connect_backend(&root, &LspContext::default())) {
        Ok(backend) => backend,
        Err(error) => {
            tracing::warn!(%error, "LSP backend unavailable before initialize; using stdio");
            rt.shutdown_timeout(Duration::from_secs(2));
            return standalone_with_initial(connection, initial);
        }
    };
    let result = rt.block_on(relay_lsp(connection, backend, root, initial, params, parsed));
    rt.shutdown_timeout(Duration::from_secs(2));
    result
}

fn standalone_with_initial(connection: Connection, initial: Message) -> Result<()> {
    let (sender, receiver) = crossbeam_channel::bounded(32);
    sender.send(initial)?;
    let incoming = connection.receiver;
    std::thread::Builder::new().name("bsl-lsp-fallback-input".into()).spawn(move || {
        while let Ok(message) = incoming.recv() {
            if sender.send(message).is_err() {
                break;
            }
        }
    })?;
    bsl_analyzer::main_loop(Connection { sender: connection.sender, receiver })
}

struct BackendPipe {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    messages: tokio::sync::mpsc::Receiver<Message>,
    controls: tokio::sync::mpsc::Receiver<LspControl>,
    reader: tokio::task::JoinHandle<Result<()>>,
    _send_control: tokio::sync::mpsc::Sender<LspControl>,
}

impl Drop for BackendPipe {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

enum RelayEvent {
    Control(Option<LspControl>),
    Backend(Option<Message>),
    Client(Option<Message>),
}

async fn next_relay_event(
    controls: &mut tokio::sync::mpsc::Receiver<LspControl>,
    backend: &mut tokio::sync::mpsc::Receiver<Message>,
    client: &mut tokio::sync::mpsc::Receiver<Message>,
    controls_open: bool,
    prefer_client: bool,
) -> RelayEvent {
    // Retirement must precede EOF. Alternate ready data directions so either
    // continuous producer still allows progress on the other side.
    if prefer_client {
        tokio::select! {
            biased;
            event = controls.recv(), if controls_open => RelayEvent::Control(event),
            event = client.recv() => RelayEvent::Client(event),
            event = backend.recv() => RelayEvent::Backend(event),
        }
    } else {
        tokio::select! {
            biased;
            event = controls.recv(), if controls_open => RelayEvent::Control(event),
            event = backend.recv() => RelayEvent::Backend(event),
            event = client.recv() => RelayEvent::Client(event),
        }
    }
}

async fn connect_backend(root: &Path, context: &LspContext) -> Result<BackendPipe> {
    let root = root.canonicalize()?;
    let project = mcp_server::project::at(&root)?;
    let cache = mcp_server::WorkspaceCacheLayout::for_project_in_current_dir(&project, None, None)?;
    shared::prepare_platform_identity(&root)?;
    let identity = shared::identity(&root, &cache)?;
    let key = mcp_server::broker::SharedBackendKey::new(identity.clone());
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .args(["mcp", "serve", "--profile", "workspace", "--mode", "daemon", "--source-dir"])
        .arg(&root)
        .arg("--cache-dir")
        .arg(cache.declared_base())
        .env(mcp_server::WORKSPACE_CACHE_SCOPE_ENV, cache.scope_stamp())
        .env(shared::AUTO_DAEMON_ENV, "1")
        .env_remove("BSL_ONEC_PASSWORD")
        .env_remove("BSL_ONEC_CONNECTIONS_FILE")
        .env_remove(mcp_server::WORKSPACE_CACHE_BASE_ENV);
    let mut stream = mcp_server::broker::proxy::connect_or_launch_stream(key, command).await?;
    let bytes = serde_json::to_vec(context)?;
    shared::attach(&mut stream, identity, AdapterKind::Lsp, &bytes).await?;
    let channel = LspChannel::new(stream, || {});
    if context.initialize_params.is_some() {
        channel.send_control(LspControl::ResumeLsp(bytes)).await?;
    }
    let (io, controls, send_control) = channel.into_parts();
    let (read, write) = tokio::io::split(io);
    let (sender, messages) = tokio::sync::mpsc::channel(16);
    let reader = tokio::spawn(async move {
        let mut read = tokio::io::BufReader::new(read);
        while let Some(message) = shared::read_lsp_message(&mut read).await? {
            if sender.send(message).await.is_err() {
                break;
            }
        }
        Ok(())
    });
    Ok(BackendPipe { write, messages, controls, reader, _send_control: send_control })
}

async fn relay_lsp(
    connection: Connection,
    mut backend: BackendPipe,
    root: PathBuf,
    initial: Message,
    initialize_params: serde_json::Value,
    parsed: lsp_types::InitializeParams,
) -> Result<()> {
    let encoding = bsl_analyzer::lsp::PositionEncoding::negotiate(&parsed.capabilities);
    let mut documents = bsl_analyzer::mem_docs::MemDocs::new();
    let mut pending = HashMap::<RequestId, String>::new();
    if let Message::Request(request) = &initial {
        pending.insert(request.id.clone(), request.method.clone());
    }
    shared::write_lsp_message(&mut backend.write, &initial).await?;
    let (sender, mut incoming) = tokio::sync::mpsc::channel(16);
    let receiver = connection.receiver;
    tokio::task::spawn_blocking(move || {
        while let Ok(message) = receiver.recv() {
            if sender.blocking_send(message).is_err() {
                break;
            }
        }
    });
    let mut next_server_request_id = 1i32;
    let mut progress_generation = 0u64;
    let mut initialized = false;
    let mut announced_capabilities = None;
    let mut pending_server = HashSet::<RequestId>::new();
    let mut pending_rebind_refresh = HashSet::<RequestId>::new();
    let mut active_progress = HashMap::<String, serde_json::Value>::new();
    let mut awaiting_rebind_ready = false;
    let rebind_refresh_capabilities =
        RebindRefreshCapabilities::from_initialize(&initialize_params);
    let mut shutdown_requested = false;
    let mut controls_open = true;
    let mut prefer_client = false;
    let result = async { loop {
        match next_relay_event(&mut backend.controls, &mut backend.messages, &mut incoming, controls_open, prefer_client).await {
            RelayEvent::Control(control) => match control {
                Some(LspControl::ScopeRetired { .. }) => {
                    cancel_pending(&connection.sender, &mut pending);
                    if !initialized || shutdown_requested { break Err(anyhow::anyhow!("workspace retired during initialization or shutdown")); }
                    if awaiting_rebind_ready { break Err(anyhow::anyhow!("workspace retired again before the rebound session became ready")); }
                    awaiting_rebind_ready = true;
                    pending_rebind_refresh.clear();
                    for id in pending_server.drain() {
                        connection.sender.send(Message::Notification(lsp_server::Notification::new(
                            "$/cancelRequest".into(), serde_json::json!({"id": id}),
                        )))?;
                    }
                    for (_, token) in active_progress.drain() {
                        connection.sender.send(Message::Notification(lsp_server::Notification::new(
                            "$/progress".into(), serde_json::json!({"token": token, "value": {"kind": "end"}}),
                        )))?;
                    }
                    progress_generation = progress_generation.saturating_add(1);
                    let context = LspContext { initialize_params: Some(initialize_params.clone()), announced_capabilities: announced_capabilities.clone(), next_server_request_id, progress_generation };
                    backend = match connect_backend(&root, &context).await {
                        Ok(backend) => backend,
                        Err(error) => break Err(error.context("LSP workspace rebind failed")),
                    };
                    controls_open = true;
                    for uri in documents.uris() {
                        let params = lsp_types::DidOpenTextDocumentParams { text_document: lsp_types::TextDocumentItem {
                            uri: uri.clone(), language_id: "bsl".into(),
                            version: documents.get_version(&uri).unwrap_or_default(),
                            text: documents.get(&uri).unwrap_or_default(),
                        }};
                        shared::write_lsp_message(&mut backend.write, &Message::Notification(lsp_server::Notification::new("textDocument/didOpen".into(), params))).await?;
                    }
                }
                Some(LspControl::ResumeLsp(_)) => break Err(anyhow::anyhow!("unexpected daemon resume control")),
                None => controls_open = false,
            },
            RelayEvent::Backend(message) => {
                prefer_client = true;
                match message {
                Some(message) => {
                    if awaiting_rebind_ready && confirms_rebind_publication(&message, &documents) {
                        awaiting_rebind_ready = false;
                    }
                    match &message {
                        Message::Response(response) => {
                            let Some(method) = pending.remove(&response.id) else { continue };
                            if method == "initialize" {
                                initialized = response.error.is_none();
                                announced_capabilities = response.result.as_ref().map(|result| result["capabilities"].clone());
                            }
                        },
                        Message::Request(request) => {
                            pending_server.insert(request.id.clone());
                            if awaiting_rebind_ready
                                && is_supported_rebind_refresh(
                                    &request.method,
                                    rebind_refresh_capabilities,
                                )
                            {
                                pending_rebind_refresh.insert(request.id.clone());
                            }
                            if let Some(id) = serde_json::to_value(&request.id)?.as_i64() {
                                next_server_request_id = next_server_request_id.max(i32::try_from(id)?.checked_add(1).context("server request id overflow")?);
                            }
                        },
                        Message::Notification(notification) => {
                            if notification.method == "$/progress" {
                                let token = notification.params["token"].clone();
                                let key = serde_json::to_string(&token)?;
                                match notification.params["value"]["kind"].as_str() {
                                    Some("begin") => { active_progress.insert(key, token); },
                                    Some("end") => { active_progress.remove(&key); },
                                    _ => {},
                                }
                            }
                        },
                    }
                    connection.sender.send(message)?;
                }
                None if shutdown_requested => break Ok(()),
                None => break Err(anyhow::anyhow!("LSP backend disconnected; session cannot be replayed")),
                }
            },
            RelayEvent::Client(message) => {
                prefer_client = false;
                match message {
                Some(message) => {
                    match &message {
                        Message::Request(request) => {
                            shutdown_requested |= request.method == "shutdown";
                            pending.insert(request.id.clone(), request.method.clone());
                        },
                        Message::Notification(notification) => mirror_document(&mut documents, notification, encoding),
                        Message::Response(response) => {
                            if !pending_server.remove(&response.id) { continue; }
                            if rebind_refresh_completed(response, &mut pending_rebind_refresh) {
                                awaiting_rebind_ready = false;
                            }
                        },
                    }
                    shared::write_lsp_message(&mut backend.write, &message).await?;
                }
                None => break Ok(()),
                }
            },
        }
    }}.await;
    cancel_pending(&connection.sender, &mut pending);
    result
}

#[derive(Clone, Copy)]
struct RebindRefreshCapabilities {
    semantic_tokens: bool,
    workspace_diagnostics: bool,
}

impl RebindRefreshCapabilities {
    fn from_initialize(params: &serde_json::Value) -> Self {
        Self {
            semantic_tokens: params["capabilities"]["workspace"]["semanticTokens"]
                ["refreshSupport"]
                .as_bool()
                == Some(true),
            workspace_diagnostics: ["diagnostics", "diagnostic"].iter().any(|key| {
                params["capabilities"]["workspace"][key]["refreshSupport"].as_bool() == Some(true)
            }),
        }
    }
}

fn confirms_rebind_publication(
    message: &Message,
    documents: &bsl_analyzer::mem_docs::MemDocs,
) -> bool {
    match message {
        Message::Notification(notification)
            if notification.method == "textDocument/publishDiagnostics" =>
        {
            let Some(uri) =
                notification.params["uri"].as_str().and_then(|uri| lsp_types::Url::parse(uri).ok())
            else {
                return false;
            };
            let Some(version) = documents.get_version(&uri) else { return false };
            notification.params["version"].as_i64() == Some(i64::from(version))
                || (notification.params["version"].is_null()
                    && notification.params["diagnostics"].as_array().is_some_and(Vec::is_empty))
        }
        _ => false,
    }
}

fn is_supported_rebind_refresh(method: &str, capabilities: RebindRefreshCapabilities) -> bool {
    match method {
        "workspace/semanticTokens/refresh" => capabilities.semantic_tokens,
        "workspace/diagnostic/refresh" => capabilities.workspace_diagnostics,
        _ => false,
    }
}

fn rebind_refresh_completed(
    response: &lsp_server::Response,
    pending: &mut HashSet<RequestId>,
) -> bool {
    pending.remove(&response.id) && response.error.is_none()
}

fn cancel_pending(
    sender: &crossbeam_channel::Sender<Message>,
    pending: &mut HashMap<RequestId, String>,
) {
    for (id, _) in pending.drain() {
        let response = lsp_server::Response::new_err(
            id,
            lsp_server::ErrorCode::ContentModified as i32,
            "Workspace changed or backend disconnected; request cancelled".into(),
        );
        let _ = sender.send(Message::Response(response));
    }
}

fn mirror_document(
    documents: &mut bsl_analyzer::mem_docs::MemDocs,
    notification: &lsp_server::Notification,
    encoding: bsl_analyzer::lsp::PositionEncoding,
) {
    // Like the standalone dispatcher, a notification whose params do not parse is logged
    // and skipped: it must not end the session.
    fn params<P: serde::de::DeserializeOwned>(
        notification: &lsp_server::Notification,
    ) -> Option<P> {
        serde_json::from_value(notification.params.clone())
            .inspect_err(|error| {
                tracing::error!(
                    method = notification.method,
                    %error,
                    "failed to parse notification params"
                );
            })
            .ok()
    }

    match notification.method.as_str() {
        "textDocument/didOpen" => {
            let Some(params) = params::<lsp_types::DidOpenTextDocumentParams>(notification) else {
                return;
            };
            let doc = params.text_document;
            if doc.uri.to_file_path().is_ok_and(|path| project_model::is_bsl_source_path(&path)) {
                documents.insert(doc.uri, doc.text, doc.version);
            }
        }
        "textDocument/didChange" => {
            let Some(params) = params::<lsp_types::DidChangeTextDocumentParams>(notification)
            else {
                return;
            };
            let uri = params.text_document.uri;
            let _ = documents.update_versioned(
                &uri,
                params.content_changes,
                encoding,
                params.text_document.version,
            );
        }
        "textDocument/didClose" => {
            let Some(params) = params::<lsp_types::DidCloseTextDocumentParams>(notification) else {
                return;
            };
            documents.remove(&params.text_document.uri);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relay_services_both_ready_data_producers_and_prioritizes_retirement() {
        let (control_tx, mut controls) = tokio::sync::mpsc::channel(1);
        let (backend_tx, mut backend) = tokio::sync::mpsc::channel(1);
        let (client_tx, mut client) = tokio::sync::mpsc::channel(1);
        let message = || {
            Message::Notification(lsp_server::Notification::new(
                "test".into(),
                serde_json::Value::Null,
            ))
        };
        backend_tx.send(message()).await.unwrap();
        client_tx.send(message()).await.unwrap();
        assert!(matches!(
            next_relay_event(&mut controls, &mut backend, &mut client, true, false).await,
            RelayEvent::Backend(Some(_))
        ));
        backend_tx.send(message()).await.unwrap();
        assert!(matches!(
            next_relay_event(&mut controls, &mut backend, &mut client, true, true).await,
            RelayEvent::Client(Some(_))
        ));
        client_tx.send(message()).await.unwrap();
        control_tx
            .send(LspControl::ScopeRetired { epoch: 1, reason: "test".into() })
            .await
            .unwrap();
        assert!(matches!(
            next_relay_event(&mut controls, &mut backend, &mut client, true, true).await,
            RelayEvent::Control(Some(LspControl::ScopeRetired { .. }))
        ));
        assert!(matches!(
            next_relay_event(&mut controls, &mut backend, &mut client, false, false).await,
            RelayEvent::Backend(Some(_))
        ));
    }

    #[test]
    fn resume_buffers_keep_the_current_text_and_wire_version() {
        let root = tempfile::tempdir().unwrap();
        let uri = lsp_types::Url::from_file_path(root.path().join("Модуль.bsl")).unwrap();
        let mut docs = bsl_analyzer::mem_docs::MemDocs::new();
        let encoding = bsl_analyzer::lsp::PositionEncoding::negotiate(&Default::default());
        mirror_document(
            &mut docs,
            &lsp_server::Notification::new(
                "textDocument/didOpen".into(),
                serde_json::json!({"textDocument": {"uri": uri, "languageId":"bsl", "version":17, "text":"Диск"}}),
            ),
            encoding,
        );
        mirror_document(
            &mut docs,
            &lsp_server::Notification::new(
                "textDocument/didChange".into(),
                serde_json::json!({"textDocument": {"uri": uri, "version":22}, "contentChanges":[{"text":"Несохранённый буфер"}]}),
            ),
            encoding,
        );
        assert_eq!(docs.get(&uri).as_deref(), Some("Несохранённый буфер"));
        assert_eq!(docs.get_version(&uri), Some(22));
        mirror_document(
            &mut docs,
            &lsp_server::Notification::new(
                "textDocument/didClose".into(),
                serde_json::json!({"textDocument": {"uri": uri}}),
            ),
            encoding,
        );
        assert!(docs.is_empty());
    }

    #[test]
    fn unparsable_document_notifications_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let uri = lsp_types::Url::from_file_path(root.path().join("Модуль.bsl")).unwrap();
        let mut docs = bsl_analyzer::mem_docs::MemDocs::new();
        let encoding = bsl_analyzer::lsp::PositionEncoding::negotiate(&Default::default());
        mirror_document(
            &mut docs,
            &lsp_server::Notification::new(
                "textDocument/didOpen".into(),
                serde_json::json!({"textDocument": {"uri": uri, "languageId":"bsl", "version":1, "text":"Буфер"}}),
            ),
            encoding,
        );
        for (method, params) in [
            ("textDocument/didOpen", serde_json::json!({"textDocument": {"uri": uri}})),
            (
                "textDocument/didChange",
                serde_json::json!({"textDocument": {"uri": uri, "version": 2}}),
            ),
            ("textDocument/didClose", serde_json::json!({})),
        ] {
            let notification = lsp_server::Notification::new(method.into(), params);
            mirror_document(&mut docs, &notification, encoding);
        }
        assert_eq!(docs.get(&uri).as_deref(), Some("Буфер"));
        assert_eq!(docs.get_version(&uri), Some(1));
    }

    #[test]
    fn retirement_cancels_each_accepted_request_once() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut pending = HashMap::from([
            (RequestId::from(7), "hover".into()),
            (RequestId::from("named".to_owned()), "definition".into()),
        ]);
        cancel_pending(&sender, &mut pending);
        cancel_pending(&sender, &mut pending);
        let responses = receiver.try_iter().collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert!(responses.iter().all(|message| matches!(message, Message::Response(response) if response.error.as_ref().unwrap().code == lsp_server::ErrorCode::ContentModified as i32)));
    }

    #[test]
    fn rebind_readiness_uses_current_publication_or_supported_refresh() {
        let root = tempfile::tempdir().unwrap();
        let uri = lsp_types::Url::from_file_path(root.path().join("Модуль.bsl")).unwrap();
        let other_uri =
            lsp_types::Url::from_file_path(root.path().join("Исключенный.bsl")).unwrap();
        let mut documents = bsl_analyzer::mem_docs::MemDocs::new();
        documents.insert(uri.clone(), "source".to_owned(), 12);
        documents.insert(other_uri.clone(), "excluded".to_owned(), 4);
        let no_refresh =
            RebindRefreshCapabilities { semantic_tokens: false, workspace_diagnostics: false };
        let refresh =
            RebindRefreshCapabilities { semantic_tokens: true, workspace_diagnostics: true };

        let current = Message::Notification(lsp_server::Notification::new(
            "textDocument/publishDiagnostics".into(),
            serde_json::json!({"uri": uri, "version": 12, "diagnostics": []}),
        ));
        assert!(confirms_rebind_publication(&current, &documents));

        let stale = Message::Notification(lsp_server::Notification::new(
            "textDocument/publishDiagnostics".into(),
            serde_json::json!({"uri": uri, "version": 11, "diagnostics": []}),
        ));
        assert!(!confirms_rebind_publication(&stale, &documents));

        let excluded = Message::Notification(lsp_server::Notification::new(
            "textDocument/publishDiagnostics".into(),
            serde_json::json!({"uri": other_uri, "diagnostics": []}),
        ));
        assert!(confirms_rebind_publication(&excluded, &documents));

        let semantic_refresh = Message::Request(lsp_server::Request::new(
            RequestId::from(9),
            "workspace/semanticTokens/refresh".into(),
            serde_json::Value::Null,
        ));
        let semantic_request = match &semantic_refresh {
            Message::Request(request) => request,
            _ => unreachable!(),
        };
        assert!(!is_supported_rebind_refresh(&semantic_request.method, no_refresh));
        assert!(is_supported_rebind_refresh(&semantic_request.method, refresh));

        let refresh_id = RequestId::from(10);
        let mut pending_refresh = HashSet::from([refresh_id.clone()]);
        let rejected = lsp_server::Response::new_err(
            refresh_id.clone(),
            lsp_server::ErrorCode::RequestFailed as i32,
            "refresh rejected".into(),
        );
        assert!(!rebind_refresh_completed(&rejected, &mut pending_refresh));
        assert!(pending_refresh.is_empty());

        let successful_id = RequestId::from(11);
        let mut pending_refresh = HashSet::from([successful_id.clone()]);
        let success = lsp_server::Response::new_ok(successful_id, serde_json::Value::Null);
        assert!(rebind_refresh_completed(&success, &mut pending_refresh));

        let window_message = Message::Notification(lsp_server::Notification::new(
            "window/showMessage".into(),
            serde_json::json!({"type": 1, "message": "ready? no"}),
        ));
        assert!(!confirms_rebind_publication(&window_message, &documents));
    }
}
