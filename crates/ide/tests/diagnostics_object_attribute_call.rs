//! A method called on an attribute of the module's own object.
//!
//! In an object module the bare name of an attribute denotes that attribute
//! (`ЭтотОбъект.Реквизит1`). An attribute without a type (empty `<Type/>`) or
//! with a type the analyzer does not lower (`Отбор`, `ЛюбаяСсылка`) infers to
//! Unknown; the call on it must stay silent like the same call on an untyped
//! form attribute, and must not be reported as an unresolved receiver.

use hir::{InferenceDiagnostic, UnresolvedMethodKind};
use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use std::path::Path;
use tempfile::TempDir;
use vfs::{FileId, FileSet, VfsPath};

const DATA_PROCESSOR_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" xmlns:v8="http://v8.1c.ru/8.1/data/core" xmlns:cfg="http://v8.1c.ru/8.1/data/enterprise/current-config" version="2.10">
    <DataProcessor uuid="22222222-2222-2222-2222-222222222222">
        <Properties>
            <Name>Обработка1</Name>
            <DefaultForm>DataProcessor.Обработка1.Form.Форма</DefaultForm>
        </Properties>
        <ChildObjects>
            <Attribute uuid="33333333-3333-3333-3333-333333333301">
                <Properties>
                    <Name>РеквизитБезТипа</Name>
                    <Type/>
                </Properties>
            </Attribute>
            <Attribute uuid="33333333-3333-3333-3333-333333333302">
                <Properties>
                    <Name>РеквизитОтбор</Name>
                    <Type>
                        <v8:Type>cfg:Filter</v8:Type>
                    </Type>
                </Properties>
            </Attribute>
            <Attribute uuid="33333333-3333-3333-3333-333333333303">
                <Properties>
                    <Name>РеквизитЛюбаяСсылка</Name>
                    <Type>
                        <v8:TypeSet>cfg:AnyRef</v8:TypeSet>
                    </Type>
                </Properties>
            </Attribute>
            <Form>Форма</Form>
        </ChildObjects>
    </DataProcessor>
</MetaDataObject>"#;

const FORM_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Form xmlns="http://v8.1c.ru/8.3/xcf/logform" xmlns:v8="http://v8.1c.ru/8.1/data/core" xmlns:cfg="http://v8.1c.ru/8.1/data/enterprise/current-config" version="2.10">
    <Properties>
        <Name>Форма</Name>
    </Properties>
    <Attributes>
        <Attribute name="Объект" id="1">
            <Type>
                <v8:Type>cfg:DataProcessorObject.Обработка1</v8:Type>
            </Type>
            <MainAttribute>true</MainAttribute>
        </Attribute>
    </Attributes>
</Form>"#;

const CATALOG_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" xmlns:v8="http://v8.1c.ru/8.1/data/core" xmlns:cfg="http://v8.1c.ru/8.1/data/enterprise/current-config" version="2.10">
    <Catalog uuid="55555555-5555-5555-5555-555555555555">
        <Properties>
            <Name>Справочник1</Name>
            <CodeLength>9</CodeLength>
            <DescriptionLength>25</DescriptionLength>
        </Properties>
        <ChildObjects>
            <Attribute uuid="66666666-6666-6666-6666-666666666602">
                <Properties>
                    <Name>РеквизитБезТипа</Name>
                    <Type/>
                </Properties>
            </Attribute>
            <Attribute uuid="66666666-6666-6666-6666-666666666603">
                <Properties>
                    <Name>РеквизитОтбор</Name>
                    <Type>
                        <v8:Type>cfg:Filter</v8:Type>
                    </Type>
                </Properties>
            </Attribute>
            <Attribute uuid="66666666-6666-6666-6666-666666666601">
                <Properties>
                    <Name>РеквизитЛюбаяСсылка</Name>
                    <Type>
                        <v8:TypeSet>cfg:AnyRef</v8:TypeSet>
                    </Type>
                </Properties>
            </Attribute>
        </ChildObjects>
    </Catalog>
</MetaDataObject>"#;

const CONFIGURATION_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MetaDataObject xmlns="http://v8.1c.ru/8.3/MDClasses" version="2.10">
    <Configuration uuid="11111111-1111-1111-1111-111111111111">
        <Properties>
            <Name>ObjectAttributeCallConfig</Name>
        </Properties>
        <ChildObjects>
            <Catalog>Справочник1</Catalog>
            <DataProcessor>Обработка1</DataProcessor>
        </ChildObjects>
    </Configuration>
</MetaDataObject>"#;

const DATA_PROCESSOR_OBJECT_MODULE: &str = "DataProcessors/Обработка1/Ext/ObjectModule.bsl";
const DATA_PROCESSOR_FORM_MODULE: &str =
    "DataProcessors/Обработка1/Forms/Форма/Ext/Form/Module.bsl";
const CATALOG_OBJECT_MODULE: &str = "Catalogs/Справочник1/Ext/ObjectModule.bsl";

fn write_config(root: &Path) {
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent directory"))
            .expect("create fixture directory");
        std::fs::write(path, text).expect("write fixture file");
    };
    write("Configuration.xml", CONFIGURATION_XML);
    write("DataProcessors/Обработка1.xml", DATA_PROCESSOR_XML);
    write("DataProcessors/Обработка1/Forms/Форма/Ext/Form.xml", FORM_XML);
    write("Catalogs/Справочник1.xml", CATALOG_XML);
}

/// `(receiver, kind)` of every `UnresolvedMethodCall` in `module` with body `bsl`.
fn unresolved_calls(module: &str, bsl: &str) -> Vec<(String, UnresolvedMethodKind)> {
    use hir::HirDatabase;

    let temp: TempDir = tempfile::tempdir().expect("create synthetic config tempdir");
    write_config(temp.path());
    let module_path = temp.path().join(module);
    std::fs::create_dir_all(module_path.parent().expect("module directory"))
        .expect("create module directory");

    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut file_set = FileSet::default();
    file_set.insert(file_id, VfsPath::new(module_path.to_string_lossy().as_ref()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(file_set));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, bsl);
    db.set_all_config_paths(vec![(None, temp.path().to_path_buf())], None);

    db.infer(file_id)
        .diagnostics
        .iter()
        .filter_map(|(_, d)| match d {
            InferenceDiagnostic::UnresolvedMethodCall { receiver_name, kind, .. } => {
                Some((receiver_name.as_str().to_string(), *kind))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn data_processor_untyped_attribute_method_call_silent() {
    let bsl = "Процедура Тест()\n    РеквизитБезТипа.Вставить(\"Ключ\", 1);\nКонецПроцедуры\n";
    let found = unresolved_calls(DATA_PROCESSOR_OBJECT_MODULE, bsl);
    assert!(
        found.is_empty(),
        "a call on an untyped attribute of the module's own object must stay silent, got {found:?}"
    );
}

#[test]
fn data_processor_unlowered_type_attribute_method_call_silent() {
    let bsl = "Процедура Тест()\n    \
        РеквизитОтбор.Сбросить();\n    \
        РеквизитЛюбаяСсылка.ПолучитьОбъект();\n\
        КонецПроцедуры\n";
    let found = unresolved_calls(DATA_PROCESSOR_OBJECT_MODULE, bsl);
    assert!(
        found.is_empty(),
        "a call on an attribute whose type is not lowered must stay silent, got {found:?}"
    );
}

#[test]
fn catalog_untyped_attribute_method_call_silent() {
    let bsl = "Процедура Тест()\n    РеквизитБезТипа.Вставить(\"Ключ\", 1);\nКонецПроцедуры\n";
    let found = unresolved_calls(CATALOG_OBJECT_MODULE, bsl);
    assert!(
        found.is_empty(),
        "a call on an untyped attribute of the catalog's own object must stay silent, got {found:?}"
    );
}

#[test]
fn catalog_unlowered_type_attribute_method_call_silent() {
    let bsl = "Процедура Тест()\n    \
        РеквизитОтбор.Сбросить();\n    \
        РеквизитЛюбаяСсылка.ПолучитьОбъект();\n\
        КонецПроцедуры\n";
    let found = unresolved_calls(CATALOG_OBJECT_MODULE, bsl);
    assert!(
        found.is_empty(),
        "a call on an attribute of the catalog's own object must stay silent, got {found:?}"
    );
}

#[test]
fn form_main_attribute_untyped_member_method_call_silent() {
    let bsl = "&НаСервере\nПроцедура Тест()\n    \
        Объект.РеквизитБезТипа.Вставить(\"Ключ\", 1);\n\
        КонецПроцедуры\n";
    let found = unresolved_calls(DATA_PROCESSOR_FORM_MODULE, bsl);
    assert!(
        found.is_empty(),
        "a call on an untyped attribute reached through the main form attribute must stay \
         silent, got {found:?}"
    );
}

#[test]
fn undeclared_receiver_in_object_module_still_reported() {
    // The silent arm is gated on the attribute name: a receiver that is neither
    // an attribute of the object nor anything else keeps its finding.
    let bsl = "Процедура Тест()\n    НетТакогоРеквизита.Вставить(\"Ключ\", 1);\nКонецПроцедуры\n";
    let found = unresolved_calls(DATA_PROCESSOR_OBJECT_MODULE, bsl);
    assert_eq!(found.len(), 1, "an undeclared receiver must still be reported, got {found:?}");
    assert_eq!(found[0].0, "НетТакогоРеквизита");
    assert!(
        matches!(
            found[0].1,
            UnresolvedMethodKind::ReceiverNotResolved | UnresolvedMethodKind::ReceiverNameAbsent
        ),
        "an undeclared receiver must be reported as unresolved, got {found:?}"
    );
}
