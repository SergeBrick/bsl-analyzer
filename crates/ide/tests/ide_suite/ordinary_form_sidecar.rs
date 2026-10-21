//! An ORDINARY form may come with `Ext/Form/Form.json` next to its module: a description
//! of the binary `Ext/Form.bin` dialog (controls, form attributes, bound procedures).
//! With it the module's handlers and form attributes are the form's own; without it
//! nothing changes. The sidecar format: `docs/configuration/ordinary-form-sidecar.md`.

use hir::{DefDatabase, ModuleId};
use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use std::path::PathBuf;
use vfs::{FileId, FileSet, VfsPath};

fn designer_fixture_path() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../bsl-metadata/fixtures/designer"))
}

/// An ordinary form whose dialog is described by a sidecar: controls `ДействияФормы`,
/// `Панель1`, `Кнопка1`, `Поле1` (bound `Поле1ПриИзменении`), attribute `Реквизит1`,
/// form event `ПриОткрытии`, command-bar action `ДействияФормыВыполнить`.
fn form_with_sidecar() -> PathBuf {
    designer_fixture_path()
        .join("Catalogs/Справочник1/Forms/ФормаОбычнаяСОписанием/Ext/Form/Module.bsl")
}

fn setup(disk_path: PathBuf, bsl: &str) -> (RootDatabaseImpl, FileId) {
    assert!(disk_path.exists(), "fixture missing: {}", disk_path.display());
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut file_set = FileSet::default();
    file_set.insert(file_id, VfsPath::new(disk_path.to_string_lossy().as_ref()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(file_set));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, bsl);
    db.set_all_config_paths(vec![(None, designer_fixture_path())], None);
    (db, file_id)
}

/// `(code, line, message)` of every diagnostic, lines 0-based.
fn findings(disk_path: PathBuf, code: &str) -> Vec<(DiagnosticCode, u32, String)> {
    let (db, file_id) = setup(disk_path, code);
    ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
        .into_iter()
        .map(|d| {
            let start = usize::from(d.range.start());
            let line = code[..start].matches('\n').count();
            (d.code, u32::try_from(line).unwrap(), d.message)
        })
        .collect()
}

fn lines_of(found: &[(DiagnosticCode, u32, String)], code: DiagnosticCode) -> Vec<u32> {
    found.iter().filter(|(c, _, _)| *c == code).map(|(_, line, _)| *line).collect()
}

const HANDLERS: &str = "\
Процедура Поле1ПриИзменении(Элемент)
	Реквизит1 = 1;
КонецПроцедуры

Процедура ДействияФормыВыполнить(Кнопка)
КонецПроцедуры

Процедура ПередОткрытием(Отказ, СтандартнаяОбработка)
КонецПроцедуры
";

#[test]
fn sidecar_form_knows_its_bindings_and_attributes() {
    let (db, file_id) = setup(form_with_sidecar(), HANDLERS);
    let metadata = db.module_metadata(ModuleId::new(file_id));
    assert!(metadata.is_ordinary_form_module());

    let found = findings(form_with_sidecar(), HANDLERS);
    // Bound to a control event and to a command-bar button: not unused.
    let unused = lines_of(&found, DiagnosticCode::UnusedLocalMethod);
    assert!(!unused.contains(&0) && !unused.contains(&4), "{found:?}");
    assert!(lines_of(&found, DiagnosticCode::UnusedParameters).is_empty(), "{found:?}");
    // A form attribute is no local variable.
    assert!(lines_of(&found, DiagnosticCode::UnusedLocalVariable).is_empty(), "{found:?}");
    // The sidecar lists what is bound: a procedure merely named after a form event
    // the form does not bind is unused.
    assert_eq!(unused, vec![7], "{found:?}");
}
