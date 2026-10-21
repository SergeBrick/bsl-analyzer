//! `ReadOnlyPropertyAssignment` reports only the writes the platform refuses.
//!
//! Every write below ran on 8.3.17.1549 and 8.3.27.2214: the ones in the
//! "writable" fixtures are performed and read back, the controls fail with
//! «Поле объекта недоступно для записи».

use std::path::PathBuf;

use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use vfs::{FileId, FileSet, VfsPath};

fn designer_fixture_path() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../bsl-metadata/fixtures/designer"))
}

/// The text as a managed form module of the designer fixture (catalog
/// `Справочник1`, form `ФормаЭлемента`), with the configuration loaded.
fn setup(bsl: &str) -> (RootDatabaseImpl, FileId) {
    let disk_path = designer_fixture_path()
        .join("Catalogs/Справочник1/Forms/ФормаЭлемента/Ext/Form/Module.bsl");
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

/// `(line, message)` of every `ReadOnlyPropertyAssignment`, lines 0-based.
fn read_only_findings(bsl: &str) -> Vec<(usize, String)> {
    let (db, file_id) = setup(bsl);
    let mut found: Vec<(usize, String)> =
        ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
            .into_iter()
            .filter(|d| d.code == DiagnosticCode::ReadOnlyPropertyAssignment)
            .map(|d| (bsl[..usize::from(d.range.start())].matches('\n').count(), d.message))
            .collect();
    found.sort();
    found
}

#[test]
fn writable_platform_properties_are_not_reported() {
    let code = "\
&НаКлиенте
Процедура НаКлиенте()
	Диалог = Новый ДиалогВыбораЦвета;
	Диалог.Цвет = WebЦвета.Красный;
	Кнопка = Элементы.Добавить(\"Кнопка\", Тип(\"КнопкаФормы\"));
	Кнопка.СочетаниеКлавиш = Новый СочетаниеКлавиш(Клавиша.F7);
	Таблица = Элементы.Добавить(\"Таблица\", Тип(\"ТаблицаФормы\"));
	Таблица.АвтоОтметкаНезаполненного = Истина;
	Планировщик = Новый Планировщик;
	ЭлементПланировщика = Планировщик.Элементы.Добавить(ТекущаяДата(), ТекущаяДата());
	ЭлементПланировщика.ЗначенияИзмерений = Новый ФиксированноеСоответствие(Новый Соответствие);
КонецПроцедуры

&НаСервере
Процедура НаСервере()
	Расширение = РасширенияКонфигурации.Создать();
	Расширение.ЗащитаОтОпасныхДействий = Новый ОписаниеЗащитыОтОпасныхДействий;
	Расширение.ИспользоватьОсновныеРолиДляВсехПользователей = Ложь;
	Уведомление = Новый ДоставляемоеУведомление;
	Уведомление.ЗвуковоеОповещение = ЗвуковоеОповещение.Нет;
	Элемент = Справочники.Справочник1.СоздатьЭлемент();
	Элемент.ИмяПредопределенныхДанных = \"\";
	Вид = ПланыВидовХарактеристик.ПланВидовХарактеристик1.СоздатьЭлемент();
	Вид.ИмяПредопределенныхДанных = \"\";
КонецПроцедуры
";
    assert_eq!(read_only_findings(code), Vec::<(usize, String)>::new());
}

#[test]
fn english_names_of_writable_properties_are_not_reported() {
    let code = "\
&НаКлиенте
Процедура НаКлиенте()
	Dialog = New ColorChooseDialog;
	Dialog.Color = WebColors.Red;
	Button = Items.Add(\"Кнопка\", Type(\"FormButton\"));
	Button.Shortcut = New Shortcut(Key.F7);
КонецПроцедуры
";
    assert_eq!(read_only_findings(code), Vec::<(usize, String)>::new());
}

/// Without a form context the bare names of form properties are the method's own
/// locals: the platform runs these assignments and returns their sum.
#[test]
fn form_property_names_are_locals_in_methods_without_context() {
    let code = "\
&НаСервереБезКонтекста
Функция БезКонтекстаНаСервере(Форма)
	Элементы = Форма.Элементы;
	Параметры = Новый Структура;
	УсловноеОформление = Форма.УсловноеОформление;
	Возврат Элементы;
КонецФункции

&НаКлиентеНаСервереБезКонтекста
Процедура БезКонтекста(Форма)
	Элементы = Форма.Элементы;
	Параметры = Новый Структура;
КонецПроцедуры
";
    assert_eq!(read_only_findings(code), Vec::<(usize, String)>::new());
}

#[test]
fn untyped_locals_do_not_regain_form_property_types() {
    for directive in ["НаСервереБезКонтекста", "НаКлиентеНаСервереБезКонтекста"]
    {
        let code = format!(
            "&{directive}\nФункция БезКонтекста(Форма)\n\
             Элементы = Форма.Элементы;\n\
             Возврат Элементы.Добавить(1);\nКонецФункции"
        );
        let (db, file_id) = setup(&code);
        let findings: Vec<_> =
            ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
                .into_iter()
                .filter(|d| d.code == DiagnosticCode::MismatchedArgCount)
                .collect();
        assert!(findings.is_empty(), "{directive}: {findings:?}");
    }
}

#[test]
fn chart_object_predefined_names_are_writable() {
    let code = "\
&НаСервере
Процедура ЗаписатьИмена()
    Счет = ПланыСчетов.ПланСчетов1.СоздатьСчет();
    Счет.ИмяПредопределенныхДанных = \"\";
    ВидРасчета = ПланыВидовРасчета.ПланВидовРасчета1.СоздатьВидРасчета();
    ВидРасчета.ИмяПредопределенныхДанных = \"\";
    Счет.Предопределенный = Истина;
    ВидРасчета.Предопределенный = Истина;
КонецПроцедуры
";
    let lines: Vec<_> = read_only_findings(code).into_iter().map(|(line, _)| line).collect();
    assert_eq!(lines, vec![6, 7]);
}

/// Controls: the writes the platform refuses keep their finding, the bare form
/// property names among them in a method that has the form in scope.
#[test]
fn read_only_properties_are_still_reported() {
    let code = "\
&НаКлиенте
Процедура НаКлиенте()
	ЭтаФорма.Элементы = Неопределено;
	ЭтаФорма.МодальныйРежим = Ложь;
	Для Каждого КлючИЗначение Из Новый Структура(\"А\", 1) Цикл
		КлючИЗначение.Значение = 2;
	КонецЦикла;
КонецПроцедуры

&НаСервере
Процедура НаСервере()
	Элементы = Неопределено;
	Расширение = РасширенияКонфигурации.Создать();
	Расширение.УникальныйИдентификатор = Новый УникальныйИдентификатор;
	Элемент = Справочники.Справочник1.СоздатьЭлемент();
	Элемент.Предопределенный = Истина;
КонецПроцедуры
";
    let lines: Vec<usize> = read_only_findings(code).into_iter().map(|(line, _)| line).collect();
    assert_eq!(lines, vec![2, 3, 5, 11, 13, 15], "{:?}", read_only_findings(code));
}
