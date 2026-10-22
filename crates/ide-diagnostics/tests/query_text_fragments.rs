//! A literal that is a piece of a query text, not the text itself.
//!
//! The platform executes the text a module assembles: a constant concatenation is one query,
//! a literal concatenated with a run-time value or completed further down is a piece of one,
//! a template is a query only after its placeholders are substituted. A parse error of such a
//! literal alone says nothing about the query; a complete static text with a real error is
//! still reported, in the literal that holds the error.

use ide_db::base_db::{SourceDatabase, SourceRoot, SourceRootId};
use ide_db::RootDatabaseImpl;
use ide_diagnostics::{Diagnostic, DiagnosticCode, DiagnosticsConfig};
use text_size::TextSize;
use vfs::{FileId, FileSet, VfsPath};

fn query_errors(source: &str) -> Vec<Diagnostic> {
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
        .collect()
}

fn method(body: &str) -> String {
    format!("Функция Тест(Имя, Флаг, Условие) Экспорт\n{body}\nКонецФункции\n")
}

fn assert_reported(body: &str) {
    let source = method(body);
    assert!(!query_errors(&source).is_empty(), "expected QueryParseError in\n{source}");
}

#[test]
fn completion_in_another_method_does_not_hide_an_error() {
    let source = "Функция A()\nТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\nВозврат Новый Запрос(Текст);\nКонецФункции\nФункция B(Условие)\nТекст = Текст + Условие;\nКонецФункции";
    assert!(!query_errors(source).is_empty());
}

#[test]
fn completion_of_a_different_value_does_not_hide_an_error() {
    assert_reported(
        "Текст = Текст + Условие;\nТекст = \"ВЫБРАТЬ 1 КАК\";\nВозврат Новый Запрос(Текст);",
    );
    assert_reported("Текст = \"ВЫБРАТЬ 1 КАК\";\nЗапрос = Новый Запрос(Текст);\nТекст = Текст + Условие;\nВозврат Запрос;");
    assert_reported(
        "Текст = \"ВЫБРАТЬ 1 КАК\";\nТекст = \"другое значение\";\nТекст = Текст + Условие;",
    );
    assert_reported("Текст = \"ВЫБРАТЬ 1 КАК\";\nКопия = Текст;\nТекст = Текст + Условие;\nВозврат Новый Запрос(Копия);");
    assert_reported("Текст = \"ВЫБРАТЬ 1 КАК\";\nВозврат Текст;\nТекст = Текст + Условие;");
    assert_reported("Запрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК\");\nЗапрос.Выполнить();\nЗапрос.Text = Запрос.Текст + Условие;");
    assert_reported("Запрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК\");\nЗапрос = Новый Запрос;\nЗапрос.Text = Запрос.Текст + Условие;");
}

#[test]
fn passing_a_query_object_consumes_its_current_text() {
    assert_reported("Запрос = Новый Запрос(\"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\");\nВыполнитьЗапрос(Запрос);\nЗапрос.Текст = Запрос.Текст + \" Т.Код = 1\";");
    assert_reported("Запрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК\");\nВозврат Запрос;\nЗапрос.Текст = Запрос.Текст + Условие;");
    assert_reported("Запрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК\");\nКопия = Запрос;\nЗапрос.Текст = Запрос.Текст + Условие;\nКопия.Выполнить();");
    assert_reported(
        "Текст = \"ВЫБРАТЬ 1 КАК\";\nВыполнитьЗапрос(Текст + \" \" );\nТекст = Текст + Условие;",
    );
}

#[test]
fn constant_concatenation_can_be_completed_later() {
    assert_silent("Текст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т\" + \" ГДЕ\";\nТекст = Текст + Условие;\nВозврат Новый Запрос(Текст);");
    assert_silent(
        "Запрос = Новый Запрос(\"ВЫБРАТЬ 1 \" + \" КАК\");\nЗапрос.Текст = Запрос.Текст + Условие;",
    );
    assert_silent("Возврат СтрЗаменить(\"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\" + \" //УСЛОВИЕ\", \"//УСЛОВИЕ\", Условие);");
}

#[test]
fn a_partial_text_is_not_validated_as_an_executable_query() {
    assert_silent("Возврат \"ВЫБРАТЬ ИЗ Справочник.Валюты КАК Т \" + Условие;");
    assert_silent("Возврат СтрЗаменить(\"ВЫБРАТЬ ИЗ Справочник.Валюты КАК Т\", \"&Х\", \"1\");");
    assert_silent("Текст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\nЕсли Флаг Тогда\nТекст = Текст + \" Т.Код = 1\";\nКонецЕсли;\nВозврат Новый Запрос(Текст);");
}

#[test]
fn a_sibling_branch_cannot_complete_the_literal() {
    assert_reported("Текст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т\";\nЕсли Флаг Тогда\nТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\nИначе\nТекст = Текст + \" Т.Код = 1\";\nКонецЕсли;\nВозврат Новый Запрос(Текст);");
    assert_reported("Если Флаг Тогда\nТекст = \"ВЫБРАТЬ 1 КАК\";\nИначеЕсли Условие Тогда\nТекст = Текст + Имя;\nКонецЕсли;\nВозврат Новый Запрос(Текст);");
    assert_reported("Если Флаг Тогда\nТекст = \"ВЫБРАТЬ 1 КАК Один\";\nИначеЕсли Условие Тогда\nТекст = \"ВЫБРАТЬ 1 КАК\";\nИначе\nТекст = Текст + Имя;\nКонецЕсли;\nВозврат Новый Запрос(Текст);");
    assert_silent("Если Флаг Тогда\nТекст = \"ВЫБРАТЬ 1 КАК\";\nТекст = Текст + Имя;\nИначе\nТекст = \"ВЫБРАТЬ 2\";\nКонецЕсли;\nВозврат Новый Запрос(Текст);");
    assert_silent("Если Флаг Тогда\nТекст = \"ВЫБРАТЬ 1 КАК\";\nИначе\nТекст = \"ВЫБРАТЬ 2 КАК\";\nКонецЕсли;\nТекст = Текст + Имя;\nВозврат Новый Запрос(Текст);");
}

#[test]
fn unrelated_concatenation_does_not_complete_the_query() {
    assert_reported("Текст = \"ВЫБРАТЬ 1 КАК\";\nОтладочнаяСтрока = Текст + \" хвост\";\nВозврат Новый Запрос(Текст);");
}

#[test]
fn character_prefix_is_constant_and_keeps_error_locations() {
    for prefix in ["Символы.ПС", "Символы.ВК", "Символы.Таб", "Chars.LF", "Chars.CR", "Chars.Tab"]
    {
        assert_silent(&format!(
            "Возврат Новый Запрос({prefix} + \"ВЫБРАТЬ 1 КАК Один,\" + \" 2 КАК Два\");"
        ));
        let source = method(&format!("Возврат Новый Запрос({prefix} + \"ВЫБРАТЬ 1 КАК Один,\");"));
        let errors = query_errors(&source);
        assert!(!errors.is_empty(), "expected QueryParseError in\n{source}");
        let start = TextSize::try_from(source.find("\"ВЫБРАТЬ").unwrap()).unwrap();
        let end = TextSize::try_from(source.find("\");").unwrap()).unwrap();
        assert!(errors
            .iter()
            .all(|error| error.range.start() >= start && error.range.end() <= end));
    }
}

fn assert_silent(body: &str) {
    let source = method(body);
    let errors = query_errors(&source);
    assert!(errors.is_empty(), "no QueryParseError expected in\n{source}\ngot {errors:#?}");
}

#[test]
fn a_constant_concatenation_is_checked_as_one_query() {
    assert_silent(
        "\tЗапрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК Один,\" + \" 2 КАК Два\");\n\tВозврат Запрос;",
    );
    // A branch of `?()` holding the whole concatenation is still one constant text.
    assert_silent(
        "\tВозврат ?(Флаг, \"ВЫБРАТЬ 1 КАК Один,\" + \" 2 КАК Два\", \"ВЫБРАТЬ 3 КАК Три\");",
    );
}

#[test]
fn a_concatenation_through_line_feed_constants_is_one_query() {
    assert_silent(
        "\tТекст = \"ВЫБРАТЬ Т.Ссылка КАК Ссылка ИЗ\" + Символы.ПС\n\
         \t\t+ \"Справочник.Валюты КАК Т\" + Chars.LF + \"ГДЕ Т.Код = &Код\";\n\
         \tВозврат Новый Запрос(Текст);",
    );
}

/// The joined text is checked, so an error in a later literal is reported — inside that literal.
#[test]
fn an_error_in_the_second_literal_is_reported_in_the_second_literal() {
    let source =
        method("\tЗапрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК Один,\" + \" 2 КАК\");\n\tВозврат Запрос;");
    let errors = query_errors(&source);
    let second = source.find("\" 2 КАК\"").expect("second literal");
    let closing_quote = TextSize::try_from(second + "\" 2 КАК".len()).unwrap();
    assert_eq!(errors.len(), 1, "one error of the joined text expected, got {errors:#?}");
    assert_eq!(errors[0].range.start(), closing_quote, "{errors:#?}");
}

#[test]
fn a_literal_concatenated_with_a_run_time_value_is_a_piece() {
    assert_silent(
        "\tЗапрос = Новый Запрос(\"ВЫБРАТЬ Т.Ссылка ИЗ \" + Имя + \" КАК Т\");\n\tВозврат Запрос;",
    );
    assert_silent(
        "\tТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ \"\n\
         \t\t+ ?(Флаг, \"Т.Код = 1\", \"Т.Код = 2\");\n\tВозврат Текст;",
    );
}

#[test]
fn a_text_completed_by_a_later_concatenation_is_a_piece() {
    assert_silent(
        "\tТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\n\
         \tЕсли Флаг Тогда\n\t\tТекст = Текст + \" Т.Код = 1\";\n\tИначе\n\
         \t\tТекст = Текст + \" Т.Код = 2\";\n\tКонецЕсли;\n\tВозврат Новый Запрос(Текст);",
    );
    assert_silent(
        "\tЗапрос = Новый Запрос;\n\tЗапрос.Текст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\n\
         \tЗапрос.Текст = Запрос.Текст + Условие;\n\tВозврат Запрос;",
    );
    assert_silent(
        "\tЗапрос = Новый Запрос(\"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\");\n\
         \tЗапрос.Текст = Запрос.Текст + Условие;\n\tВозврат Запрос;",
    );
}

#[test]
fn a_template_is_a_query_only_after_substitution() {
    assert_silent(
        "\tТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ //УСЛОВИЕ\";\n\
         \tТекст = СтрЗаменить(Текст, \"//УСЛОВИЕ\", Условие);\n\tВозврат Новый Запрос(Текст);",
    );
    assert_silent("\tВозврат Новый Запрос(СтрШаблон(\"ВЫБРАТЬ Т.Ссылка ИЗ %1 КАК Т\", Имя));");
    // The substitution happens elsewhere; the placeholders alone say it is a template.
    assert_silent("\tВозврат \"ВЫБРАТЬ Т.Ссылка ИЗ #Таблица КАК Т\";");
    assert_silent(
        "\tВозврат \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ Т.Код В (%Коды%)\";",
    );
    assert_silent("\tВозврат \"ВЫБРАТЬ Т.Ссылка ИЗ [Таблица] КАК Т\";");
}

#[test]
fn a_wql_text_is_not_a_query_of_the_platform() {
    assert_silent("\tВозврат \"SELECT Name FROM Win32_Process WHERE Name = 'x'\";");
}

/// Controls, one detail away from the silent cases above.
#[test]
fn a_complete_static_text_with_an_error_is_reported() {
    let errors = query_errors(&method(
        "\tЗапрос = Новый Запрос(\"ВЫБРАТЬ 1 КАК Один,\");\n\tВозврат Запрос;",
    ));
    assert_eq!(errors.len(), 1, "{errors:#?}");

    // Stored in a variable that nothing completes: the text the query gets is the literal.
    let errors = query_errors(&method(
        "\tТекст = \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ\";\n\
         \tВозврат Новый Запрос(Текст);",
    ));
    assert_eq!(errors.len(), 1, "{errors:#?}");

    // A placeholder inside a string of the query is the string's text.
    let errors = query_errors(&method(
        "\tВозврат \"ВЫБРАТЬ Т.Ссылка ИЗ Справочник.Валюты КАК Т ГДЕ Т.Код ПОДОБНО \"\"#1%\"\" И\";",
    ));
    assert_eq!(errors.len(), 1, "{errors:#?}");
}
