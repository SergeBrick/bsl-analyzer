//! The MCP resident analyses under the same project-level semantic inputs as CLI
//! `analyze` and the LSP: `[features]`, the platform target/floor and the
//! compatibility mode all come from `bsl-analyzer.toml`, not from defaults.

use std::path::Path;

use super::lifecycle::DiagnosticsState;
use super::test_support::{
    module_diag_fingerprint, module_path, sample_workspace, wait_ready, write, write_common_module,
};

/// A client common module calling a platform function that is unavailable only in
/// the web client.
fn web_client_only_gap(root: &Path) {
    write_common_module(
        root,
        "ОбщийМодуль1",
        false,
        "Функция ПакетОтвета(Ответ) Экспорт\n    Возврат XMLСтрока(Ответ);\nКонецФункции\n",
    );
}

fn web_client_findings(root: &Path) -> Vec<String> {
    let state = DiagnosticsState::for_workspace(root.to_path_buf());
    state.ensure_loading();
    wait_ready(&state);
    let findings = module_diag_fingerprint(&state, &module_path(root, "ОбщийМодуль1"))
        .into_iter()
        .filter(|line| line.contains("UnavailableInEnvironment") && line.contains("XMLСтрока"))
        .collect();
    state.shutdown();
    findings
}

/// Control: with the default checked set the web client is checked, so the stand
/// does produce the finding the next test expects to disappear.
#[test]
fn default_checked_environments_report_the_web_client_gap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    web_client_only_gap(root);
    write(root, "bsl-analyzer.toml", "[source]\nroot = \".\"\n");

    let findings = web_client_findings(root);
    assert!(!findings.is_empty(), "the stand must reproduce the web-client finding by default");
}

#[test]
fn checked_environments_without_web_client_silence_the_web_client_gap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    web_client_only_gap(root);
    write(
        root,
        "bsl-analyzer.toml",
        "[source]\nroot = \".\"\n\n[features]\nchecked_environments = [\"ТонкийКлиент\", \
         \"ТолстыйКлиентУправляемоеПриложение\", \"Сервер\"]\n",
    );

    let findings = web_client_findings(root);
    assert!(
        findings.is_empty(),
        "an environment the project excluded must not be checked: {findings:#?}"
    );
}

#[test]
fn explicit_empty_checked_environments_disable_the_availability_check() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    web_client_only_gap(root);
    write(
        root,
        "bsl-analyzer.toml",
        "[source]\nroot = \".\"\n\n[features]\nchecked_environments = []\n",
    );

    let findings = web_client_findings(root);
    assert!(findings.is_empty(), "`[]` is the documented opt-out: {findings:#?}");
}

/// Whether the execution model of a freshly built resident supports the ordinary
/// application.
fn built_ordinary_app_support(run_mode: &str, features: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let root = &dir.path().canonicalize().unwrap();
    sample_workspace(root);
    write(
        root,
        "Configuration.xml",
        &format!(
            "<MetaDataObject><Configuration><Properties><DefaultRunMode>{run_mode}\
             </DefaultRunMode></Properties></Configuration></MetaDataObject>"
        ),
    );
    write(root, "bsl-analyzer.toml", &format!("[source]\nroot = \".\"\n{features}"));
    let resident = DiagnosticsState::build_resident_for_test(root);
    resident.db.env_options().ordinary_app_support
}

/// The execution model is built from the project as in CLI `analyze`: the legacy
/// thick client is modelled when `checked_environments` names it or `<DefaultRunMode>`
/// implies it, and not otherwise.
#[test]
fn the_resident_build_models_the_ordinary_application_like_the_cli() {
    let checked_legacy =
        "[features]\nchecked_environments = [\"ТолстыйКлиентОбычноеПриложение\"]\n";
    assert!(!built_ordinary_app_support("ManagedApplication", ""));
    assert!(built_ordinary_app_support("ManagedApplication", checked_legacy));
    assert!(built_ordinary_app_support("OrdinaryApplication", ""));
}
