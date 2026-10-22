//! Query constructs the platform executes and the query parser used to reject: a list after
//! `В ИЕРАРХИИ`, a number with an empty fraction (`0.`), a dereference of a parenthesized
//! expression (`(ВЫБОР ... КОНЕЦ).Поле`). Each is checked through both routes a query reaches
//! the rules by, with a control one detail away that the platform rejects too.

use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{Diagnostic, DiagnosticCode, DiagnosticsConfig};
use vfs::{FileId, FileSet, VfsPath};

fn module_query_errors(query: &str) -> Vec<Diagnostic> {
    let source = format!(
        "Функция Тест() Экспорт\n\tЗапрос = Новый Запрос(\"{query}\");\n\
         \tВозврат Запрос.Выполнить();\nКонецФункции\n"
    );
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut files = FileSet::default();
    files.insert(file_id, VfsPath::new("/test/Module.bsl".to_string()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, &source);
    ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
        .into_iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::QueryParseError)
        .collect()
}

fn bare_query_errors(query: &str) -> Vec<Diagnostic> {
    ide_diagnostics::validate_query_text(&DiagnosticsConfig::all_enabled(), None, query)
        .into_iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::QueryParseError)
        .collect()
}

fn assert_parses(query: &str) {
    let module = module_query_errors(query);
    assert!(module.is_empty(), "module route, {query}: {module:#?}");
    let bare = bare_query_errors(query);
    assert!(bare.is_empty(), "bare route, {query}: {bare:#?}");
}

fn assert_rejected(query: &str) {
    assert!(!module_query_errors(query).is_empty(), "module route must reject {query}");
    assert!(!bare_query_errors(query).is_empty(), "bare route must reject {query}");
}

#[test]
fn a_list_of_roots_in_hierarchy_parses() {
    assert_parses(
        "ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ Справочник.Справочник1 КАК Т \
         ГДЕ Т.Ссылка В ИЕРАРХИИ (ЗНАЧЕНИЕ(Справочник.Справочник1.ПустаяСсылка), &Группа)",
    );
    assert_parses(
        "ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ Справочник.Справочник1 КАК Т \
         ГДЕ Т.Ссылка НЕ В ИЕРАРХИИ (&Группа1, &Группа2, &Группа3)",
    );
    // The same list as a condition of a virtual table.
    assert_parses(
        "ВЫБРАТЬ Т.Счет КАК Счет ИЗ РегистрБухгалтерии.Хозрасчетный.Обороты(\
         &Начало, &Конец, , Счет В ИЕРАРХИИ (&Счет1, &Счет2), , ) КАК Т",
    );
}

#[test]
fn an_empty_list_in_hierarchy_is_rejected() {
    assert_rejected(
        "ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ Справочник.Справочник1 КАК Т ГДЕ Т.Ссылка В ИЕРАРХИИ ()",
    );
}

#[test]
fn a_number_with_an_empty_fraction_parses() {
    assert_parses("ВЫБРАТЬ 0. КАК Поле");
    assert_parses("ВЫБРАТЬ 1. + 2 КАК Поле");
}

#[test]
fn a_number_with_two_dots_is_rejected() {
    assert_rejected("ВЫБРАТЬ 0.. КАК Поле");
}

#[test]
fn a_dereference_of_parentheses_parses() {
    assert_parses(
        "ВЫБРАТЬ (ВЫБОР КОГДА ИСТИНА ТОГДА Т.Ссылка КОНЕЦ).Наименование КАК Н \
         ИЗ Справочник.Справочник1 КАК Т",
    );
    assert_parses("ВЫБРАТЬ (Т.Ссылка).Наименование КАК Н ИЗ Справочник.Справочник1 КАК Т");
    assert_parses("ВЫБРАТЬ (&Ссылка).Наименование КАК Н");
}

#[test]
fn a_dereference_of_case_without_parentheses_is_rejected() {
    assert_rejected(
        "ВЫБРАТЬ ВЫБОР КОГДА ИСТИНА ТОГДА Т.Ссылка КОНЕЦ.Наименование КАК Н \
         ИЗ Справочник.Справочник1 КАК Т",
    );
}

#[test]
fn a_dangling_dot_after_parentheses_before_an_alias_is_rejected() {
    assert_rejected("ВЫБРАТЬ (Т.Ссылка).КАК Н ИЗ Справочник.Справочник1 КАК Т");
    // A module string literal goes on to a new line only through `|`, so the line break
    // is checked on the bare route.
    let across_lines = "ВЫБРАТЬ (Т.Ссылка).\nКАК Н ИЗ Справочник.Справочник1 КАК Т";
    assert!(!bare_query_errors(across_lines).is_empty(), "bare route must reject {across_lines}");
    assert_rejected("ВЫБРАТЬ ВЫРАЗИТЬ(Т.Ссылка КАК Справочник.Справочник1).КАК Н ИЗ Справочник.Справочник1 КАК Т");
}
