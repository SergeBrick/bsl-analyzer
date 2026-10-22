//! A finding below a lone `\r` (including the `\r\r\n` sequence) is reported on the line an
//! LSP client shows it on, not on the line counted by `\n` alone.

use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use vfs::{FileId, FileSet, VfsPath};

/// `(start_line, start_column)` of the query parse error in `source`.
fn query_error_position(source: &str) -> (usize, usize) {
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut files = FileSet::default();
    files.insert(file_id, VfsPath::new("/test/Module.bsl".to_string()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, source);
    let errors: Vec<_> =
        ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
            .into_iter()
            .filter(|diagnostic| diagnostic.code == DiagnosticCode::QueryParseError)
            .collect();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let output = errors[0].to_output(source);
    (output.start_line, output.start_column)
}

/// The module with `gap` between its first two statements; the query error sits on the
/// statement after the gap, at column 77 of its line.
fn module(gap: &str) -> String {
    format!(
        "Функция Тест() Экспорт\r\n\tА = 1;{gap}\tЗапрос = Новый Запрос(\"ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ Справочник.Валюты КАК В\");\r\n\
         \tВозврат Запрос.Выполнить();\r\nКонецФункции\r\n"
    )
}

#[test]
fn crlf_control_reports_the_statement_line() {
    assert_eq!(query_error_position(&module("\r\n")), (2, 77));
}

#[test]
fn lone_cr_moves_the_finding_one_line_down() {
    assert_eq!(query_error_position(&module("\r")), (2, 77));
    assert_eq!(query_error_position(&module("\r\tБ = 2;\r\n")), (3, 77));
}

fn blank_line_runs(source: &str) -> Vec<(usize, usize)> {
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut files = FileSet::default();
    files.insert(file_id, VfsPath::new("/test/Module.bsl".to_string()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, source);
    ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
        .into_iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::ConsecutiveEmptyLines)
        .map(|diagnostic| {
            let output = diagnostic.to_output(source);
            (output.start_line, output.end_line)
        })
        .collect()
}

/// The platform reads `\r\r\n` as one line break, so the line an LSP client shows between
/// its `\r` and `\r\n` is no blank line of the module.
#[test]
fn cr_before_crlf_adds_no_blank_lines() {
    let one_blank = "Процедура А()\r\r\n\r\r\nКонецПроцедуры\r\n";
    assert_eq!(blank_line_runs(one_blank), []);

    // Control: two blank lines by the platform's count.
    let two_blank = "Процедура А()\r\r\n\r\r\n\r\r\nКонецПроцедуры\r\n";
    assert_eq!(blank_line_runs(two_blank), [(2, 5)]);
}

#[test]
fn cr_before_crlf_counts_as_two_line_breaks() {
    assert_eq!(query_error_position(&module("\r\r\n")), (3, 77));
    assert_eq!(query_error_position(&module("\r\r\n\tБ = 2;\r\r\n")), (5, 77));
}

fn incorrect_line_breaks(source: &str) -> usize {
    let mut db = RootDatabaseImpl::new();
    let file_id = FileId(0);
    let mut files = FileSet::default();
    files.insert(file_id, VfsPath::new("/test/Module.bsl".to_string()));
    db.set_source_root(SourceRootId(0), SourceRoot::new_local(files));
    db.set_file_source_root(file_id, SourceRootId(0));
    db.set_file_text(file_id, source);
    ide_diagnostics::file_diagnostics(&db, file_id, &DiagnosticsConfig::all_enabled())
        .into_iter()
        .filter(|diagnostic| diagnostic.code == DiagnosticCode::IncorrectLineBreak)
        .count()
}

/// The platform reads `\r\r\n` as one line break, so a literal continued after it
/// still follows the operator directly.
#[test]
fn literal_continued_after_cr_before_crlf_follows_the_operator() {
    let module = |gap: &str| {
        format!(
            "Процедура А()\r\n\tТекст = \"А\" +{gap}\t\"Б\";\r\n\tСообщить(Текст);\r\nКонецПроцедуры\r\n"
        )
    };
    assert_eq!(incorrect_line_breaks(&module("\r\r\n")), 0);

    // Control: a real empty line between them leaves the operator dangling.
    assert_eq!(incorrect_line_breaks(&module("\r\n\r\n")), 1);
}
