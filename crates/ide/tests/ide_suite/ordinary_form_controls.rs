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
        "\tИмя = \"ЭлементыФормы\"; ЭтаФорма[Имя].Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tЭтаФорма[\"Элементы\" + \"Формы\"].Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);\n",
        "\tПостроитьКнопки(ThisForm[CollectionName]);\n",
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

#[test]
fn generated_table_columns_leave_the_controls_open() {
    let root = tempfile::tempdir().unwrap();
    let form = "Catalogs/Справочник1/Forms/ФормаОбычнаяСОписанием";
    for relative in [
        "Configuration.xml".to_string(),
        "Catalogs/Справочник1.xml".to_string(),
        format!("{form}.xml"),
    ] {
        let target = root.path().join(&relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(designer_fixture_path().join(relative), target).unwrap();
    }
    let directory = root.path().join(form).join("Ext/Form");
    std::fs::create_dir_all(&directory).unwrap();
    let mut sidecar: serde_json::Value = serde_json::from_str(include_str!(
        "../../../bsl-metadata/fixtures/designer/Catalogs/Справочник1/Forms/ФормаОбычнаяСОписанием/Ext/Form/Form.json"
    ))
    .unwrap();
    sidecar["controls"].as_array_mut().unwrap().push(serde_json::json!({
        "kind": "table", "id": 5, "name": "Товары"
    }));
    std::fs::write(directory.join("Form.json"), sidecar.to_string()).unwrap();

    let mut db = RootDatabaseImpl::new();
    let file = FileId(0);
    let mut files = FileSet::default();
    files.insert(file, VfsPath::new(directory.join("Module.bsl").to_string_lossy().as_ref()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file, SourceRootId(0));
    db.set_all_config_paths(vec![(None, root.path().to_path_buf())], None);
    for (construction, expected_missing) in [
        ("", 1),
        ("ЭлементыФормы.Товары.СоздатьКолонки();", 0),
        ("Controls.Товары.CreateColumns();", 0),
    ] {
        db.set_file_text(file, &format!(
            "Процедура ПриОткрытии()\n{construction}\nЭлементыФормы.Товары.Видимость = Истина;\nЭлементыФормы.ТоварыНоменклатура.Видимость = Ложь;\nКонецПроцедуры\n"
        ));
        let found = ide_diagnostics::file_diagnostics(&db, file, &DiagnosticsConfig::all_enabled());
        let missing: Vec<_> =
            found.iter().filter(|d| d.code == DiagnosticCode::UnresolvedField).collect();
        assert_eq!(missing.len(), expected_missing, "{construction}: {missing:?}");
    }
}

#[test]
fn extension_bodies_can_add_controls_to_the_base_dialog() {
    let root = tempfile::tempdir().unwrap();
    let base = "Процедура ПриОткрытии()\nКонецПроцедуры\n";
    let mut db = RootDatabaseImpl::new();
    let main_file = FileId(0);
    let ext_file = FileId(1);
    let mut files = FileSet::default();
    files.insert(main_file, VfsPath::new(form_with_sidecar().to_string_lossy().as_ref()));
    let relative = form_with_sidecar().strip_prefix(designer_fixture_path()).unwrap().to_path_buf();
    files.insert(ext_file, VfsPath::new(root.path().join(relative).to_string_lossy().as_ref()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    for file in [main_file, ext_file] {
        db.set_file_source_root(file, SourceRootId(0));
    }
    db.set_file_text(main_file, base);
    db.set_all_config_paths(
        vec![(None, designer_fixture_path()), (Some("X".to_string()), root.path().to_path_buf())],
        Some(bsl_metadata::ExtensionScope::InfoBase),
    );
    use hir::DefDatabase;
    assert_eq!(db.module_index(SourceRootId(0)).sibling_bodies(main_file), vec![ext_file]);
    for (construction, expected_missing) in [
        ("", 1),
        ("ЭлементыФормы.Добавить(Тип(\"Кнопка\"), \"Кнопка2\", Истина);", 0),
        ("ПостроитьКнопки(ЭтаФорма);", 0),
    ] {
        db.set_file_text(ext_file, &format!(
            "&ИзменениеИКонтроль(\"ПриОткрытии\")\nПроцедура Расш_ПриОткрытии()\n#Вставка\n{construction}\nЭлементыФормы.Кнопка2.Доступность = Ложь;\n#КонецВставки\nКонецПроцедуры\n"
        ));
        assert!(ide_db::effective_target(&db, ext_file).is_some());
        let found =
            ide_diagnostics::file_diagnostics(&db, ext_file, &DiagnosticsConfig::all_enabled());
        let missing: Vec<_> =
            found.iter().filter(|d| d.code == DiagnosticCode::UnresolvedField).collect();
        assert_eq!(missing.len(), expected_missing, "{construction}: {missing:?}");
    }
}
