//! `ФабрикаXDTO.Создать` returns an `ОбъектXDTO` for an object type and a
//! `ЗначениеXDTO` for a value type, so its result is their union and a write to
//! a property of a created object is not the read-only `ЗначениеXDTO.Значение`.
//!
//! Checked on 8.3.17.1549 and 8.3.27.2214: `ТипЗнч(Ф.Создать(<ТипОбъектаXDTO>))`
//! is «Объект XDTO» and `Объект.Значение = …` / `Объект.value = …` succeed;
//! `ТипЗнч(Ф.Создать(<ТипЗначенияXDTO>, "x"))` is «Значение XDTO», and its
//! `Значение = …` fails with «Поле объекта недоступно для записи (Значение)».

use hir::{HirDatabase, TypeKernelDb, TypeKind};
use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use vfs::{FileId, FileSet, VfsPath};

fn setup(code: &str) -> (RootDatabaseImpl, FileId) {
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut file_set = FileSet::default();
    file_set.insert(file_id, VfsPath::new("/test.bsl"));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(file_set));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, code);
    (db, file_id)
}

/// `(line, message)` of every `ReadOnlyPropertyAssignment`, lines 0-based.
fn read_only_findings(code: &str) -> Vec<(usize, String)> {
    let (db, file_id) = setup(code);
    let mut found: Vec<(usize, String)> =
        ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
            .into_iter()
            .filter(|d| d.code == DiagnosticCode::ReadOnlyPropertyAssignment)
            .map(|d| (code[..usize::from(d.range.start())].matches('\n').count(), d.message))
            .collect();
    found.sort();
    found
}

/// Names of the platform types the local `name` is inferred to hold.
fn platform_type_names(code: &str, name: &str) -> Vec<String> {
    let (db, file_id) = setup(code);
    let ty = *db.infer(file_id).var_types.get(name).expect("the local is typed");
    let arms = match db.lookup_type(ty) {
        TypeKind::Union(arms) => arms.to_vec(),
        _ => vec![ty],
    };
    let mut names: Vec<String> = arms
        .iter()
        .filter_map(|arm| match db.lookup_type(*arm) {
            TypeKind::PlatformObject(facet) => Some(facet.name.to_string()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

#[test]
fn xdto_factory_create_returns_object_or_value() {
    let code = "\
Процедура Тест(ТипXDTO)
    Объект = ФабрикаXDTO.Создать(ТипXDTO);
КонецПроцедуры
";
    let names = platform_type_names(code, "объект");
    assert!(
        names.iter().any(|name| name == "ОбъектXDTO" || name == "XDTODataObject"),
        "an object type creates an ОбъектXDTO, got {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "ЗначениеXDTO" || name == "XDTODataValue"),
        "a value type creates a ЗначениеXDTO, got {names:?}"
    );
}

#[test]
fn properties_of_a_created_xdto_object_are_writable() {
    let code = "\
Процедура Тест()
    Тип = ФабрикаXDTO.Тип(\"urn:test\", \"Объект1\");
    Объект = ФабрикаXDTO.Создать(Тип);
    Объект.Значение = \"а\";
    Объект.value = \"б\";
    Объект.Value = \"в\";
    Объект.Свойство1 = 5;
КонецПроцедуры
";
    assert_eq!(read_only_findings(code), Vec::<(usize, String)>::new());
}

#[test]
fn value_of_a_known_xdto_value_is_still_read_only() {
    // Control: the default of an XDTO property can only be a ЗначениеXDTO, and
    // its Значение stays read-only.
    let code = "\
Процедура Тест(Чтение)
    Объект = ФабрикаXDTO.ПрочитатьXML(Чтение);
    ПоУмолчанию = Объект.ВладеющееСвойство().ЗначениеПоУмолчанию;
    ПоУмолчанию.Значение = 1;
КонецПроцедуры
";
    let findings = read_only_findings(code);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].0, 3);
    assert!(findings[0].1.contains("ЗначениеXDTO"), "{findings:?}");
}

fn type_mismatch_findings(code: &str) -> Vec<(usize, String)> {
    let (db, file_id) = setup(code);
    let mut found: Vec<(usize, String)> =
        ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
            .into_iter()
            .filter(|d| d.code == DiagnosticCode::TypeMismatch)
            .map(|d| (code[..usize::from(d.range.start())].matches('\n').count(), d.message))
            .collect();
    found.sort();
    found
}

#[test]
fn a_created_value_still_fits_parameters_of_one_xdto_kind() {
    // The union does not turn the parameters that admit only one of the two
    // kinds into a type mismatch: the kind is chosen by the `Тип` argument.
    let code = "\
Процедура Тест(ТипЗначения, ТипОбъекта)
    Значение = ФабрикаXDTO.Создать(ТипЗначения, 1);
    Приведенное = ФабрикаXDTO.Привести(ТипЗначения, Значение);
    Объект = ФабрикаXDTO.Создать(ТипОбъекта);
    ТипОбъекта.Проверить(Объект);
    Прочитанное = СериализаторXDTO.ПрочитатьXDTO(Объект);
КонецПроцедуры
";
    assert_eq!(type_mismatch_findings(code), Vec::<(usize, String)>::new());
}
