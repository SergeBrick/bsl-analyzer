//! Code after a lone `\r` that follows a `//` comment on the same `\n`-line is code: the
//! platform compiles it, so the analyzer must analyse it too. A `\r\n` or `\r\r\n` after the
//! comment changes nothing.

use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use vfs::{FileId, FileSet, VfsPath};

/// Number of query parse errors reported for `source`.
fn query_errors(source: &str) -> usize {
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut files = FileSet::default();
    files.insert(file_id, VfsPath::new("/test/Module.bsl".to_string()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, source);
    ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
        .into_iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::QueryParseError)
        .count()
}

/// A comment, then `after_comment`, then a statement whose query has a parse error.
fn module(after_comment: &str) -> String {
    format!(
        "Функция Тест() Экспорт\r\n\t// комментарий{after_comment}\tЗапрос = Новый Запрос(\"ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ Справочник.Валюты КАК В\");\r\n\
         \tВозврат Запрос.Выполнить();\r\nКонецФункции\r\n"
    )
}

#[test]
fn code_after_lone_cr_in_comment_is_analysed() {
    assert_eq!(query_errors(&module("\r")), 1);
}

#[test]
fn crlf_after_comment_control() {
    assert_eq!(query_errors(&module("\r\n")), 1);
}

#[test]
fn cr_cr_lf_after_comment_control() {
    assert_eq!(query_errors(&module("\r\r\n")), 1);
}

#[test]
fn without_cr_the_statement_is_part_of_the_comment() {
    assert_eq!(query_errors(&module(" ")), 0);
}
