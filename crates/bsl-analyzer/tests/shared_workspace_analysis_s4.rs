mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::Lsp;
use mcp_server::{serve_stream, McpProfile, McpServer, SharedState};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Map, Value};

type Client = RunningService<RoleClient, ()>;

const OBJECT_MODULE: &str = include_str!(
    "fixtures/shared_workspace_analysis_s4/Catalogs/КонтрольныйКаталог/Ext/ObjectModule.bsl"
);
const CATALOG_XML: &str =
    include_str!("fixtures/shared_workspace_analysis_s4/Catalogs/КонтрольныйКаталог.xml");
const XML_STRING_TYPE: &str = "<v8:Type>xs:string</v8:Type>";
const XML_NUMBER_TYPE: &str = "<v8:Type>xs:decimal</v8:Type>";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("fixture workspace");
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/shared_workspace_analysis_s4");
    for entry in walkdir::WalkDir::new(&fixture_root) {
        let entry = entry.expect("fixture entry");
        let relative = entry.path().strip_prefix(&fixture_root).expect("fixture-relative path");
        let target = root.path().join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).expect("fixture directory");
        } else {
            std::fs::create_dir_all(target.parent().expect("fixture parent"))
                .expect("fixture parent directory");
            std::fs::copy(entry.path(), target).expect("fixture file");
        }
    }
    root
}

async fn mcp_client(root: &Path) -> Client {
    let server = McpServer::new(
        McpProfile::Workspace,
        SharedState::workspace(root.to_path_buf()).expect("valid fixture workspace"),
    );
    let (client_io, server_io) = tokio::io::duplex(4 * 1024 * 1024);
    tokio::spawn(serve_stream(server, server_io));
    ().serve(client_io).await.expect("real rmcp handshake")
}

fn args(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect()
}

fn position(text: &str, needle: &str) -> (u32, u32) {
    let (line, source_line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .expect("position token exists");
    let byte_column = source_line.find(needle).expect("position token on line");
    let column = source_line[..byte_column].encode_utf16().count();
    (line as u32, column as u32)
}

async fn wait_mcp_method_type(
    client: &Client,
    path: &str,
    method: &str,
    expected: &str,
    stale: Option<&str>,
    allow_loading: bool,
) -> Value {
    let (line, column) = position(OBJECT_MODULE, method);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "MCP symbol_info never returned {expected}");
        let call =
            tokio::time::timeout(
                remaining,
                client.call_tool(CallToolRequestParams::new("symbol_info").with_arguments(args(
                    &[("path", json!(path)), ("line", json!(line)), ("column", json!(column))],
                ))),
            )
            .await
            .expect("MCP symbol_info deadline")
            .expect("MCP symbol_info transport");
        let card = call.structured_content.expect("symbol_info structured result");
        match card["status"].as_str() {
            Some("loading") => assert!(allow_loading, "MCP workspace regressed to loading"),
            Some("ok") => {
                let actual = card["return_type"].as_str().expect("method return_type");
                if actual == expected {
                    return card;
                }
                assert_eq!(Some(actual), stale, "unexpected method card: {card}");
            }
            _ => panic!("symbol_info did not resolve the local method: {card}"),
        }
        assert!(tokio::time::Instant::now() < deadline, "MCP symbol_info timeout");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn wait_lsp_hover_type(
    lsp: &mut Lsp,
    first_id: u64,
    params: Value,
    expected: &str,
    stale: Option<&str>,
) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let mut id = first_id;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!remaining.is_zero(), "LSP hover never returned {expected}");
        lsp.send(json!({
            "jsonrpc": "2.0", "id": id, "method": "textDocument/hover", "params": params
        }));
        let response = lsp
            .wait_for_within(remaining, |message| {
                message["id"] == id
                    && message["method"].is_null()
                    && (message.get("result").is_some() || message.get("error").is_some())
            })
            .unwrap_or_else(|| panic!("LSP hover request {id} exceeded its deadline"));
        assert!(response["error"].is_null(), "LSP hover failed: {response}");
        let result = response["result"].to_string();
        if result.contains(expected) {
            return response;
        }
        let stale_type = stale.expect("unexpected LSP hover result");
        assert!(result.contains(stale_type), "unexpected LSP hover response: {response}");
        assert!(std::time::Instant::now() < deadline, "LSP hover timeout");
        id += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn signal_xml_change(lsp: &mut Lsp, xml_path: &Path) {
    lsp.send(json!({
        "jsonrpc": "2.0", "method": "workspace/didChangeWatchedFiles",
        "params": {"changes": [{
            "uri": lsp_types::Url::from_file_path(xml_path).expect("XML file URI"),
            "type": 2
        }]}
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn xml_attribute_type_refreshes_dependent_lsp_and_mcp_semantics() {
    let root = fixture();
    let object_relative = PathBuf::from("Catalogs/КонтрольныйКаталог/Ext/ObjectModule.bsl");
    let object_path = root.path().join(&object_relative);
    let xml_path = root.path().join("Catalogs/КонтрольныйКаталог.xml");
    let object_uri = lsp_types::Url::from_file_path(&object_path).expect("object module URI");
    let bsl_before = std::fs::read(&object_path).expect("read fixture BSL");

    let mut lsp = Lsp::start(root.path());
    lsp.open(&object_path, OBJECT_MODULE);
    let client = mcp_client(root.path()).await;
    let hover_params = |needle: &str| {
        let (line, character) = position(OBJECT_MODULE, needle);
        json!({
            "textDocument": {"uri": object_uri.clone()},
            "position": {"line": line, "character": character}
        })
    };

    let changed_before = wait_mcp_method_type(
        &client,
        object_relative.to_str().expect("UTF-8 fixture path"),
        "ПолучитьПоле",
        "Строка",
        None,
        true,
    )
    .await;
    assert_eq!(changed_before["return_type"], "Строка");
    let stable_before = wait_mcp_method_type(
        &client,
        object_relative.to_str().expect("UTF-8 fixture path"),
        "ПолучитьКонтрольноеПоле",
        "Строка",
        None,
        false,
    )
    .await;
    assert_eq!(stable_before["return_type"], "Строка");
    wait_lsp_hover_type(&mut lsp, 2, hover_params("ИзменяемыйТип"), "Строка", None);
    wait_lsp_hover_type(&mut lsp, 3, hover_params("СтабильныйТип"), "Строка", None);

    let changed_xml = CATALOG_XML.replacen(XML_STRING_TYPE, XML_NUMBER_TYPE, 1);
    assert_ne!(changed_xml, CATALOG_XML, "fixture contains the expected XML type");
    std::fs::write(&xml_path, &changed_xml).expect("change only the metadata XML type");
    signal_xml_change(&mut lsp, &xml_path);

    let changed_after = wait_mcp_method_type(
        &client,
        object_relative.to_str().expect("UTF-8 fixture path"),
        "ПолучитьПоле",
        "Число",
        Some("Строка"),
        false,
    )
    .await;
    assert_eq!(changed_after["return_type"], "Число");
    let stable_after = wait_mcp_method_type(
        &client,
        object_relative.to_str().expect("UTF-8 fixture path"),
        "ПолучитьКонтрольноеПоле",
        "Строка",
        None,
        false,
    )
    .await;
    assert_eq!(stable_after["return_type"], "Строка");
    wait_lsp_hover_type(&mut lsp, 4, hover_params("ИзменяемыйТип"), "Число", Some("Строка"));
    wait_lsp_hover_type(&mut lsp, 5, hover_params("СтабильныйТип"), "Строка", None);
    assert_eq!(std::fs::read(&object_path).expect("BSL unchanged"), bsl_before);

    let restored_xml = changed_xml.replacen(XML_NUMBER_TYPE, XML_STRING_TYPE, 1);
    std::fs::write(&xml_path, restored_xml).expect("restore metadata XML type");
    signal_xml_change(&mut lsp, &xml_path);
    wait_mcp_method_type(
        &client,
        object_relative.to_str().expect("UTF-8 fixture path"),
        "ПолучитьПоле",
        "Строка",
        Some("Число"),
        false,
    )
    .await;
    wait_lsp_hover_type(&mut lsp, 6, hover_params("ИзменяемыйТип"), "Строка", Some("Число"));
    assert_eq!(std::fs::read(&object_path).expect("BSL unchanged after restore"), bsl_before);

    client.cancel().await.ok();
    assert!(lsp.shutdown(7).success(), "LSP exits cleanly");
}
