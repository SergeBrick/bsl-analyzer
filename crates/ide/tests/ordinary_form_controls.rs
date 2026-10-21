//! `ЭлементыФормы.<Имя>` in an ordinary form whose dialog is known from its
//! `Ext/Form/Form.json` sidecar: a control the dialog lacks is reported, unless the
//! module may add controls itself. Without a sidecar nothing is judged.

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

/// The same kind of form with no sidecar: only its descriptor is readable.
fn form_without_sidecar() -> PathBuf {
    designer_fixture_path().join("Catalogs/Справочник1/Forms/ФормаОбычная/Ext/Form/Module.bsl")
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

const CONTROLS: &str = "\
Процедура ПриОткрытии()
	ЭлементыФормы.Кнопка1.Доступность = Ложь;
	ЭлементыФормы.Кнопка2.Доступность = Ложь;
	Видна = ЭлементыФормы.Поле1.Видимость;
	Сообщить(Видна);
	Всего = ЭлементыФормы.Количество();
	Сообщить(Всего);
КонецПроцедуры
";

#[test]
fn sidecar_form_reports_a_control_its_dialog_does_not_have() {
    let found = findings(form_with_sidecar(), CONTROLS);
    assert_eq!(lines_of(&found, DiagnosticCode::UnresolvedField), vec![2], "{found:?}");
    let message = &found.iter().find(|(c, _, _)| *c == DiagnosticCode::UnresolvedField).unwrap().2;
    assert!(message.contains("Кнопка2"), "{message}");
}

#[test]
fn without_sidecar_controls_are_not_judged() {
    let found = findings(form_without_sidecar(), CONTROLS);
    assert!(lines_of(&found, DiagnosticCode::UnresolvedField).is_empty(), "{found:?}");
}

/// A module that adds controls, or hands the form or its controls to other code (which
/// may add some), can reach controls the dialog does not list.
#[test]
fn a_module_that_may_add_controls_is_not_judged() {
    let judged = [
        "\tЭлементыФормы.Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tПостроитьКнопки(ЭтаФорма);\n",
        "\tПостроитьКнопки(ЭлементыФормы);\n",
        "\tЭтаФорма.ЭлементыФормы.Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tПостроитьКнопки(ЭтаФорма.ЭлементыФормы);\n",
        "\tПостроитьКнопки(ThisForm.Controls);\n",
        "\tФ = ЭтаФорма.ЭлементыФормы;\n\tФ.Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tПостроитьКнопки(ЭтаФорма[\"ЭлементыФормы\"]);\n",
        "\tЭтаФорма[\"ЭлементыФормы\"].Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tВыполнить(\"ЭлементыФормы.Добавить(Тип(\"\"Кнопка\"\"), \"\"Кнопка2\"\", Истина)\");\n",
        "\tВычислить(\"ЭлементыФормы.Добавить(Тип(\"\"Кнопка\"\"), \"\"Кнопка2\"\", Истина)\");\n",
    ]
    .into_iter()
    .filter(|prelude| {
        let code = format!(
            "Процедура ПриОткрытии()\n{prelude}\tЭлементыФормы.Кнопка2.Доступность = Ложь;\nКонецПроцедуры\n\nПроцедура ПостроитьКнопки(Что)\nКонецПроцедуры\n"
        );
        let found = findings(form_with_sidecar(), &code);
        !lines_of(&found, DiagnosticCode::UnresolvedField).is_empty()
    })
    .collect::<Vec<_>>();
    assert!(judged.is_empty(), "judged although they may add controls: {judged:#?}");
}
