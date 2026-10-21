use crate::define_metadata;
use crate::metadata::*;
use crate::{Diagnostic, DiagnosticCode, DiagnosticsContext};

pub const METADATA: DiagnosticMetadata = define_metadata! {
    diagnostic_type: DiagnosticType::Error,
    severity: DiagnosticSeverityLevel::Critical,
    scope: DiagnosticScope::Bsl,
    modules: &[],
    minutes_to_fix: 10,
    activated_by_default: true,
    compatibility_mode: DiagnosticCompatibilityMode::Undefined,
    tags: &[MetadataTag::Sql, MetadataTag::Error],
    can_locate_on_project: false,
    extra_min_for_complexity: 0.0,
    lsp_severity_override: "",
};

/// A parameter in the source list of a statement without `ПОМЕСТИТЬ`: the platform selects an
/// external source only into a temporary table, so executing such a text fails. Only a text
/// the method executes as written is checked; a template rewritten later is not.
pub fn check(ctx: &DiagnosticsContext) -> Vec<Diagnostic> {
    let code = DiagnosticCode::QueryParameterSourceWithoutInto;
    if ctx.is_disabled_with_metadata(code) {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    crate::sdbl_utils::for_each_executed_query(ctx, |_, package, mapper, query_text| {
        for source_use in package.parameter_sources().iter().filter(|u| !u.statement_has_into) {
            diagnostics.push(Diagnostic {
                code,
                message: format!(
                    "Параметр &{} указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его \
                     содержимое только во временную таблицу. Поместите данные во временную \
                     таблицу отдельным запросом пакета и читайте её в следующем запросе",
                    source_use.source.name
                ),
                severity: ctx.config.severity(code),
                range: mapper.map_range(source_use.source.range, query_text),
                tags: ctx.config.tags(code),
                fixes: vec![],
            });
        }
    });
    diagnostics
}

#[cfg(test)]
mod tests {
    use expect_test::expect;

    use crate::test_utils::check_diagnostics_snapshot_for;
    use crate::DiagnosticCode;

    /// The reference form: a value table selected from without `ПОМЕСТИТЬ`. Every "silent"
    /// fixture carries it, so a rule that went silent everywhere cannot pass for correct.
    const CONTROL: &str = r#"
Процедура Контроль()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const I1: &str = r#"
Процедура И1()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const ISSUE: &str = r#"
Функция ПроверитьВнешнийИсточник() Экспорт
    Данные = Новый ТаблицаЗначений;
    Данные.Колонки.Добавить("Ключ", Новый ОписаниеТипов("Число"));
    Данные.Колонки.Добавить("Значение", Новый ОписаниеТипов("Число"));
    СтрокаДанных = Данные.Добавить();
    СтрокаДанных.Ключ = 1;
    СтрокаДанных.Значение = 10;

    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Возврат Запрос.Выполнить();
КонецФункции
"#;

    const NO_TYPE: &str = r#"
Процедура И3(Данные)
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const ENGLISH: &str = r#"
Процедура И4()
    Запрос = Новый Запрос;
    Запрос.Текст = "SELECT D.K AS K FROM &Data AS D";
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const JOIN: &str = r#"
Процедура И5()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Н.Ссылка КАК Ссылка ИЗ Справочник.Номенклатура КАК Н ЛЕВОЕ СОЕДИНЕНИЕ &Данные КАК Д ПО Н.Ссылка = Д.Ссылка";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const UNION: &str = r#"
Процедура Объединение()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Д.Ключ КАК Ключ ИЗ &Данные КАК Д ОБЪЕДИНИТЬ ВСЕ ВЫБРАТЬ Д.Ключ ИЗ &Данные КАК Д";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура ОбъединениеСПоместить()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Д.Ключ КАК Ключ ПОМЕСТИТЬ ВТ ИЗ &Данные КАК Д ОБЪЕДИНИТЬ ВСЕ ВЫБРАТЬ Д.Ключ ИЗ &Данные КАК Д";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const IN_LIST: &str = r#"
Процедура И10()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Н.Ссылка КАК Ссылка ИЗ Справочник.Номенклатура КАК Н ГДЕ Н.Ссылка В (&Список)";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const VIRTUAL_TABLE: &str = r#"
Процедура И11()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ К.Курс КАК Курс ИЗ РегистрСведений.КурсыВалют.СрезПоследних(&Период) КАК К";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const REASSIGNED: &str = r#"
Процедура И12а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = СтрЗаменить(Запрос.Текст, "&Данные", "Справочник.Номенклатура");
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И12б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = ТекстЗапроса();
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Функция ТекстЗапроса()
    Возврат "";
КонецФункции
"#;

    const PASSED_TO_CALL: &str = r#"
Процедура ОбновитьТекст(Параметр)
КонецПроцедуры

Процедура И13а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    ОбновитьТекст(Запрос.Текст);
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И13б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    ОбновитьТекст(Запрос);
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const NOT_A_LITERAL: &str = r#"
Процедура И14а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.Текст = Текст;
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Процедура И14б()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные" + "";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const NOT_EXECUTED: &str = r#"
Процедура И15а()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
КонецПроцедуры

Процедура И15б()
    Построитель = Новый ПостроительЗапроса;
    Построитель.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Построитель.Выполнить();
КонецПроцедуры
"#;

    #[test]
    fn fires_on_value_table_parameter_source() {
        let code = I1.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn fires_on_issue_function_with_totals_without_into() {
        let code = ISSUE.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 16:11..16:18
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn fires_without_proving_the_parameter_type() {
        let code = NO_TYPE.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 4:53..4:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn fires_on_english_syntax() {
        let code = ENGLISH.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 4:42..4:47
                  message: Параметр &Data указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn fires_on_parameter_source_in_join() {
        let code = JOIN.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:99..5:106
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn into_in_any_union_member_covers_the_statement() {
        let code = format!("{CONTROL}{UNION}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical
                QueryParameterSourceWithoutInto @ 13:48..13:55
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical
                QueryParameterSourceWithoutInto @ 13:95..13:102
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_on_parameter_in_value_position() {
        let code = format!("{CONTROL}{IN_LIST}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_on_virtual_table_argument() {
        let code = format!("{CONTROL}{VIRTUAL_TABLE}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_when_the_text_is_reassigned() {
        let code = format!("{CONTROL}{REASSIGNED}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_when_the_text_is_passed_to_a_call() {
        let code = format!("{CONTROL}{PASSED_TO_CALL}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_when_the_text_is_not_a_direct_literal() {
        let code = format!("{CONTROL}{NOT_A_LITERAL}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_without_an_executed_query_object() {
        let code = format!("{CONTROL}{NOT_EXECUTED}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    const CONSTRUCTOR_TEXT: &str = r#"
Процедура ТекстВКонструкторе()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос("ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные");
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const OUTSIDE_THE_METHOD: &str = r#"
Запрос = Новый Запрос;
Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
Запрос.Выполнить();

Процедура Подготовить(Запрос)
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
КонецПроцедуры

Процедура Выполнить(Запрос)
    Запрос.Выполнить();
КонецПроцедуры
"#;

    #[test]
    fn silent_when_the_text_is_a_constructor_argument() {
        let code = format!("{CONTROL}{CONSTRUCTOR_TEXT}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_outside_the_method_that_executes_the_text() {
        let code = format!("{OUTSIDE_THE_METHOD}{CONTROL}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 17:53..17:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    const DYNAMIC_BRANCH: &str = r#"
Процедура ВетвьСтроитТекстДинамически(Флаг)
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Если Флаг Тогда
        Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Иначе
        Запрос.Текст = ТекстЗапроса();
    КонецЕсли;
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры

Функция ТекстЗапроса()
    Возврат "";
КонецФункции
"#;

    #[test]
    fn silent_when_one_branch_builds_the_text_dynamically() {
        let code = format!("{CONTROL}{DYNAMIC_BRANCH}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    #[test]
    fn silent_when_the_code_is_disabled() {
        use crate::test_utils::check_file_diagnostics_with_config;
        use crate::DiagnosticsConfig;

        let mut disabled = DiagnosticsConfig::all_enabled();
        disabled.disabled.push(DiagnosticCode::QueryParameterSourceWithoutInto);

        let silenced = check_file_diagnostics_with_config(I1, disabled);
        let enabled = check_file_diagnostics_with_config(I1, DiagnosticsConfig::all_enabled());

        assert!(
            enabled.iter().any(|d| d.code == DiagnosticCode::QueryParameterSourceWithoutInto),
            "the fixture must report the rule when it is enabled, got {enabled:?}",
        );
        assert!(
            silenced.iter().all(|d| d.code != DiagnosticCode::QueryParameterSourceWithoutInto),
            "a disabled rule must not report, got {silenced:?}",
        );
    }
}
