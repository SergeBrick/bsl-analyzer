use crate::define_metadata;
use crate::metadata::*;
use crate::{Diagnostic, DiagnosticCode, DiagnosticsConfig, DiagnosticsContext};
use sdbl_hir;

pub const METADATA: DiagnosticMetadata = define_metadata! {
    diagnostic_type: DiagnosticType::Error,
    severity: DiagnosticSeverityLevel::Blocker,
    scope: DiagnosticScope::Bsl,
    modules: &[],
    minutes_to_fix: 5,
    activated_by_default: true,
    compatibility_mode: DiagnosticCompatibilityMode::Undefined,
    tags: &[MetadataTag::Error, MetadataTag::Sql],
    can_locate_on_project: false,
    extra_min_for_complexity: 0.0,
    lsp_severity_override: "",
};

pub(crate) fn dispatch(
    config: &DiagnosticsConfig,
    diag: &sdbl_hir::SdblDiagnostic,
    mapper: &crate::sdbl_utils::SdblPositionMapper,
    query_text: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let sdbl_hir::SdblDiagnostic::InvalidVirtualTableParameters { table_name, problem, range } =
        diag
    {
        let code = DiagnosticCode::InvalidVirtualTableParameters;
        diagnostics.push(Diagnostic {
            code,
            message: sdbl_hir::invalid_virtual_table_parameters_message(table_name, problem),
            severity: config.severity(code),
            range: mapper.map_range(*range, query_text),
            tags: config.tags(code),
            fixes: vec![],
        });
    }
}

pub fn check(ctx: &DiagnosticsContext) -> Vec<Diagnostic> {
    crate::sdbl_utils::collect_sdbl_via_dispatch(
        ctx,
        DiagnosticCode::InvalidVirtualTableParameters,
        dispatch,
    )
}

#[cfg(test)]
mod tests {
    use crate::standalone_query::validate_query_text;
    use crate::test_utils::format_diags;
    use crate::{DiagnosticCode, DiagnosticsConfig};
    use expect_test::{expect, Expect};

    /// The parameter lists are fixed by the register KIND, which the query text names, so the
    /// rule needs no metadata: it is checked here without a resolver on purpose.
    fn check(query: &str, expected: Expect) {
        let config = DiagnosticsConfig::all_enabled();
        let diagnostics: Vec<_> = validate_query_text(&config, None, query)
            .into_iter()
            .filter(|d| d.code == DiagnosticCode::InvalidVirtualTableParameters)
            .collect();
        expected.assert_eq(&format_diags(query, &diagnostics));
    }

    #[test]
    fn a_condition_split_by_a_comma_overflows_the_parameter_list() {
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки(&Период, Счет = &Счет, , Субконто1 = &Значение1, Субконто2 = &Значение2) КАК Т",
            expect![[r#"
                InvalidVirtualTableParameters @ 1:137..1:159
                  message: Неверные параметры виртуальной таблицы "РегистрБухгалтерии.Бухгалтерия1.Остатки": передано 5, а у таблицы 4 (Период, УсловиеСчета, Субконто, Условие). Условия внутри одного параметра соединяются через И, а не запятой
                  severity: Blocker"#]],
        );
    }

    #[test]
    fn an_account_condition_in_the_ext_dimension_slot_is_a_condition_where_a_value_goes() {
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки(&Период, , Счет = &Счет, ) КАК Т",
            expect![[r#"
                InvalidVirtualTableParameters @ 1:99..1:111
                  message: Неверные параметры виртуальной таблицы "РегистрБухгалтерии.Бухгалтерия1.Остатки": в параметр 3 (Субконто) передано условие, а он принимает значение. Проверьте позицию аргумента: параметры читаются строго по порядку
                  severity: Blocker"#]],
        );
    }

    #[test]
    fn a_parenthesized_condition_in_a_value_slot_is_still_a_condition() {
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки(&Период, Счет = &Счет, (Субконто1 = &Значение), ) КАК Т",
            expect![[r#"
                InvalidVirtualTableParameters @ 1:111..1:134
                  message: Неверные параметры виртуальной таблицы "РегистрБухгалтерии.Бухгалтерия1.Остатки": в параметр 3 (Субконто) передано условие, а он принимает значение. Проверьте позицию аргумента: параметры читаются строго по порядку
                  severity: Blocker"#]],
        );
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки(&Период, Счет = &Счет, (Субконто1 В (&Значение)), ) КАК Т",
            expect![[r#"
                InvalidVirtualTableParameters @ 1:111..1:136
                  message: Неверные параметры виртуальной таблицы "РегистрБухгалтерии.Бухгалтерия1.Остатки": в параметр 3 (Субконто) передано условие, а он принимает значение. Проверьте позицию аргумента: параметры читаются строго по порядку
                  severity: Blocker"#]],
        );
    }

    #[test]
    fn a_well_formed_call_is_silent() {
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки(&Период, Счет = &Счет, , Субконто1 = &Значение1 И Субконто2 = &Значение2) КАК Т",
            expect![[r#""#]],
        );
        check(
            "ВЫБРАТЬ Т.Ресурс1Оборот КАК К ИЗ РегистрНакопления.Остатки1.Обороты(&Начало, &Конец, Месяц, Измерение1 = &Значение1) КАК Т",
            expect![[r#""#]],
        );
        check(
            "ВЫБРАТЬ Т.Период КАК П ИЗ РегистрБухгалтерии.Бухгалтерия1.ДвиженияССубконто(&Начало, &Конец, Регистратор = &Документ, , ) КАК Т",
            expect![[r#""#]],
        );
        check(
            "ВЫБРАТЬ Т.Ресурс1Остаток КАК Ресурс1Остаток ИЗ РегистрБухгалтерии.Бухгалтерия1.Остатки((&Период), Счет = &Счет, , ) КАК Т",
            expect![[r#""#]],
        );
    }

    #[test]
    fn the_same_suffix_on_a_stored_table_is_not_a_virtual_table_call() {
        check("ВЫБРАТЬ Т.Реквизит1 КАК Н ИЗ Документ.Документ1.Остатки КАК Т", expect![[r#""#]]);
    }
}
