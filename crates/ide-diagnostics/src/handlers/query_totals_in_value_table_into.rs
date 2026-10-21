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

/// `ИТОГИ` in a statement putting a value table into a temporary table, which the platform
/// forbids. The parameter must be proven a value table: a tabular section or a query result
/// may be put there with totals.
pub fn check(ctx: &DiagnosticsContext) -> Vec<Diagnostic> {
    let code = DiagnosticCode::QueryTotalsInValueTableInto;
    if ctx.is_disabled_with_metadata(code) {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    crate::sdbl_utils::for_each_executed_query(ctx, |text, package, mapper, query_text| {
        let mut reported_statements = Vec::new();
        for source_use in package.parameter_sources() {
            let Some(totals) = source_use.statement_totals else {
                continue;
            };
            if !source_use.statement_has_into || reported_statements.contains(&totals) {
                continue;
            }
            let is_value_table = text.value_table_parameters.iter().any(|parameter| {
                stdx::case::eq_ignore_case(parameter.as_str(), &source_use.source.name)
            });
            if !is_value_table {
                continue;
            }
            reported_statements.push(totals);
            diagnostics.push(Diagnostic {
                code,
                message: format!(
                    "ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу \
                     таблицу значений &{}: вычислите итоги отдельным запросом пакета, читающим \
                     эту временную таблицу",
                    source_use.source.name
                ),
                severity: ctx.config.severity(code),
                range: mapper.map_range(totals, query_text),
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

    /// A value table put into a temporary table with `ИТОГИ`: the reference form of this rule.
    /// A fixture that expects silence carries it, so a rule that went silent everywhere cannot
    /// pass for correct.
    const CONTROL_TOTALS: &str = r#"
Функция Контроль() Экспорт
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
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

    /// A value table selected from without `ПОМЕСТИТЬ`: the reference form of the sibling rule,
    /// for fixtures that expect this rule silent and the sibling not.
    const CONTROL_SOURCE: &str = r#"
Процедура Контроль()
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Данные.Ключ КАК Ключ ИЗ &Данные КАК Данные";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const TOTALS_PER_STATEMENT: &str = r#"
Функция ПоместитьИИтоги() Экспорт
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Возврат Запрос.Выполнить();
КонецФункции

Функция ДваИсточника() Экспорт
    Данные = Новый ТаблицаЗначений;
    Данные2 = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |	ЛЕВОЕ СОЕДИНЕНИЕ &Данные2 КАК Д2
        |	ПО Данные.Ключ = Д2.Ключ
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.УстановитьПараметр("Данные2", Данные2);
    Возврат Запрос.Выполнить();
КонецФункции
"#;

    const NO_PROVEN_VALUE_TABLE: &str = r#"
Функция ПараметрМетода(Данные) Экспорт
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Возврат Запрос.Выполнить();
КонецФункции

Функция ДвеУстановки() Экспорт
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.УстановитьПараметр("Данные", Новый ДеревоЗначений);
    Возврат Запрос.Выполнить();
КонецФункции

Функция ЧерезПараметры() Экспорт
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст =
        "ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.Параметры.Вставить("Данные", Новый ДеревоЗначений);
    Возврат Запрос.Выполнить();
КонецФункции
"#;

    const TWO_STATEMENT_BATCH: &str = r#"
Функция ПроверитьЧерезВременнуюТаблицу() Экспорт
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
        |ПОМЕСТИТЬ ВТ_Данные
        |ИЗ
        |	&Данные КАК Данные
        |;
        |
        |ВЫБРАТЬ
        |	Данные.Ключ КАК Ключ,
        |	Данные.Значение КАК Значение
        |ИЗ
        |	ВТ_Данные КАК Данные
        |ИТОГИ
        |	СУММА(Значение)
        |ПО
        |	Ключ";
    Запрос.УстановитьПараметр("Данные", Данные);
    Возврат Запрос.Выполнить();
КонецФункции
"#;

    #[test]
    fn fires_once_per_statement_on_totals() {
        let code = TOTALS_PER_STATEMENT.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#"
                QueryTotalsInValueTableInto @ 12:10..12:15
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &Данные: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical
                QueryTotalsInValueTableInto @ 33:10..33:15
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &Данные: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn silent_without_a_proven_value_table() {
        let code = format!("{CONTROL_TOTALS}{NO_PROVEN_VALUE_TABLE}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#"
                QueryTotalsInValueTableInto @ 12:10..12:15
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &Данные: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn silent_on_the_two_statement_batch_of_the_issue() {
        let code = format!("{CONTROL_SOURCE}{TWO_STATEMENT_BATCH}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#""#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#"
                QueryParameterSourceWithoutInto @ 5:53..5:60
                  message: Параметр &Данные указан источником запроса без ПОМЕСТИТЬ: платформа выбирает его содержимое только во временную таблицу. Поместите данные во временную таблицу отдельным запросом пакета и читайте её в следующем запросе
                  severity: Critical"#]],
        );
    }

    const ENGLISH_QUERY: &str = r#"
Процедура АнглийскийЗапрос()
    ВТ = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "SELECT D.K AS K INTO TT FROM &VT AS D TOTALS SUM(K) BY K";
    Запрос.УстановитьПараметр("VT", ВТ);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const COMPUTED_PARAMETER_NAME: &str = r#"
Процедура ИмяВычисляется(ИмяПараметра)
    Данные = Новый ТаблицаЗначений;
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Д.К КАК К ПОМЕСТИТЬ ВТ ИЗ &Данные КАК Д ИТОГИ СУММА(К) ПО К";
    Запрос.УстановитьПараметр("Данные", Данные);
    Запрос.УстановитьПараметр(ИмяПараметра, Новый ДеревоЗначений);
    Запрос.Выполнить();
КонецПроцедуры
"#;

    const BOUND_AFTER_THE_EXECUTION: &str = r#"
Процедура ПослеВыполнения()
    Запрос = Новый Запрос;
    Запрос.Текст = "ВЫБРАТЬ Д.К КАК К ПОМЕСТИТЬ ВТ ИЗ &Данные КАК Д ИТОГИ СУММА(К) ПО К";
    Запрос.Выполнить();
    Запрос.УстановитьПараметр("Данные", Новый ТаблицаЗначений);
КонецПроцедуры
"#;

    /// A binding written after the execution does not bind the parameter it executes with.
    #[test]
    fn silent_when_the_value_table_is_bound_after_the_execution() {
        let code = format!("{CONTROL_TOTALS}{BOUND_AFTER_THE_EXECUTION}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#"
                QueryTotalsInValueTableInto @ 12:10..12:15
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &Данные: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical"#]],
        );
    }

    #[test]
    fn fires_on_english_query_syntax() {
        let code = ENGLISH_QUERY.to_string();
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#"
                QueryTotalsInValueTableInto @ 5:59..5:65
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &VT: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn silent_when_a_parameter_is_bound_under_a_computed_name() {
        let code = format!("{CONTROL_TOTALS}{COMPUTED_PARAMETER_NAME}");
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryTotalsInValueTableInto,
            expect![[r#"
                QueryTotalsInValueTableInto @ 12:10..12:15
                  message: ИТОГИ нельзя использовать в запросе, который помещает во временную таблицу таблицу значений &Данные: вычислите итоги отдельным запросом пакета, читающим эту временную таблицу
                  severity: Critical"#]],
        );
        check_diagnostics_snapshot_for(
            &code,
            DiagnosticCode::QueryParameterSourceWithoutInto,
            expect![[r#""#]],
        );
    }

    #[test]
    fn silent_when_the_code_is_disabled() {
        use crate::test_utils::check_file_diagnostics_with_config;
        use crate::DiagnosticsConfig;

        let mut disabled = DiagnosticsConfig::all_enabled();
        disabled.disabled.push(DiagnosticCode::QueryTotalsInValueTableInto);

        let silenced = check_file_diagnostics_with_config(TOTALS_PER_STATEMENT, disabled);
        let enabled = check_file_diagnostics_with_config(
            TOTALS_PER_STATEMENT,
            DiagnosticsConfig::all_enabled(),
        );

        assert!(
            enabled.iter().any(|d| d.code == DiagnosticCode::QueryTotalsInValueTableInto),
            "the fixture must report the rule when it is enabled, got {enabled:?}",
        );
        assert!(
            silenced.iter().all(|d| d.code != DiagnosticCode::QueryTotalsInValueTableInto),
            "a disabled rule must not report, got {silenced:?}",
        );
    }
}
