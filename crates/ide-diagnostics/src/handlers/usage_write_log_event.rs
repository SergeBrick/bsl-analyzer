use crate::define_metadata;
use crate::metadata::*;
use crate::AnalysisContext;
use crate::{Diagnostic, DiagnosticCode};
use hir::LocalRange;

pub const METADATA: DiagnosticMetadata = define_metadata! {
    diagnostic_type: DiagnosticType::CodeSmell,
    severity: DiagnosticSeverityLevel::Info,
    scope: DiagnosticScope::Bsl,
    modules: &[],
    minutes_to_fix: 1,
    activated_by_default: true,
    compatibility_mode: DiagnosticCompatibilityMode::Undefined,
    tags: &[MetadataTag::Standard, MetadataTag::Badpractice],
    can_locate_on_project: false,
    extra_min_for_complexity: 0.0,
    lsp_severity_override: "",
};

const WRITE_LOG_EVENT_METHOD_PARAMS_COUNT: usize = 5;

#[allow(clippy::too_many_arguments)]
pub fn from_hir(
    in_except_block: bool,
    arg_count: usize,
    log_level_empty: bool,
    comment_empty: bool,
    has_error_log_level: bool,
    has_detail_error_description: bool,
    except_has_raise: bool,
    range: LocalRange,
    ctx: &AnalysisContext,
) -> Option<Diagnostic<LocalRange>> {
    let code = DiagnosticCode::UsageWriteLogEvent;

    if ctx.is_disabled_with_metadata(code) {
        return None;
    }

    if arg_count < WRITE_LOG_EVENT_METHOD_PARAMS_COUNT {
        return Some(Diagnostic {
            code,
            message: "Неверное число параметров метода".to_string(),
            severity: ctx.severity(code),
            range,
            tags: ctx.tags(code),
            fixes: vec![],
        });
    }

    if log_level_empty {
        return Some(Diagnostic {
            code,
            message: "Не указан 2й параметр с типом \"УровеньЖурналаРегистрации\"".to_string(),
            severity: ctx.severity(code),
            range,
            tags: ctx.tags(code),
            fixes: vec![],
        });
    }

    if comment_empty {
        return Some(Diagnostic {
            code,
            message: "Не указан 5й параметр \"Комментарий\"".to_string(),
            severity: ctx.severity(code),
            range,
            tags: ctx.tags(code),
            fixes: vec![],
        });
    }

    if in_except_block {
        if !has_error_log_level {
            return Some(Diagnostic {
                code,
                message: "Нужно указывать уровень \"Ошибка\" при записи в журнал регистрации внутри блока Исключение-КонецПопытки".to_string(),
                severity: ctx.severity(code),
                range,
                tags: ctx.tags(code),
                fixes: vec![],
            });
        }

        if !has_detail_error_description && !except_has_raise {
            return Some(Diagnostic {
                code,
                message: "В тексте комментария нет вызова \"ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())\"".to_string(),
                severity: ctx.severity(code),
                range,
                tags: ctx.tags(code),
                fixes: vec![],
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use crate::test_utils::check_diagnostics_snapshot_for;
    use crate::DiagnosticCode;
    use expect_test::expect;

    fn check(code: &str, expected: expect_test::Expect) {
        check_diagnostics_snapshot_for(code, DiagnosticCode::UsageWriteLogEvent, expected);
    }

    /// A bank-upload procedure whose `Исключение` branch holds `handler`.
    fn in_except(handler: &str) -> String {
        format!(
            "Процедура ВыгрузитьВБанк(Пакет, Ссылка)\n\tПопытка\n\t\tПакет.Отправить();\n\tИсключение\n{handler}\tКонецПопытки;\nКонецПроцедуры\n"
        )
    }

    // ── Обязательные параметры ─────────────────────────────────────────────

    #[test]
    fn test_correct_usage_outside_except() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка",
		УровеньЖурналаРегистрации.Предупреждение, , Ответ,
		Ответ.Текст);
КонецПроцедуры
"#;
        check(code, expect![[r#""#]]);
    }

    #[test]
    fn test_wrong_number_params() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка");
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:43
              message: Неверное число параметров метода
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_two_params_wrong_count() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка", УровеньЖурналаРегистрации.Предупреждение);
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:85
              message: Неверное число параметров метода
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_four_params_wrong_count() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка", УровеньЖурналаРегистрации.Предупреждение, , Ответ);
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:94
              message: Неверное число параметров метода
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_no_second_parameter() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка",
		,
		, Ответ, Ответ.Текст);
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..4:24
              message: Не указан 2й параметр с типом "УровеньЖурналаРегистрации"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_no_comment() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка", УровеньЖурналаРегистрации.Предупреждение, , Ответ, );
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:96
              message: Не указан 5й параметр "Комментарий"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_english_keywords() {
        let code = r#"Procedure LogBankReply(Reply)
	WriteLogEvent("Bank.Upload");
EndProcedure
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:30
              message: Неверное число параметров метода
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_case_insensitive() {
        let code = r#"Процедура ЗаписатьОтказБанка(Ответ)
	записьжурналарегистрации("Банк.Выгрузка");
КонецПроцедуры
"#;
        check(
            code,
            expect![[r#"
            UsageWriteLogEvent @ 2:2..2:43
              message: Неверное число параметров метода
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_variable_comment_outside_except() {
        let code = r#"Процедура ЗаписатьОтказБанка(Знач ИмяСобытия, Знач Ответ)
	Причина = РазобратьОтвет(Ответ);
	ЗаписьЖурналаРегистрации(
		ИмяСобытия,
		УровеньЖурналаРегистрации.Ошибка,
		,
		Ответ,
		Причина);
КонецПроцедуры
"#;
        check(code, expect![[r#""#]]);
    }

    #[test]
    fn test_parameter_named_like_detail_outside_except() {
        let code = r#"Процедура ЗаписатьОтказБанка(Знач ПодробноеПредставлениеОшибки)
	ЗаписьЖурналаРегистрации("Банк.Выгрузка",
		УровеньЖурналаРегистрации.Ошибка, , ,
		ПодробноеПредставлениеОшибки);
КонецПроцедуры
"#;
        check(code, expect![[r#""#]]);
    }

    #[test]
    fn test_error_processing_detail_in_string_concat_outside_except() {
        let code = r#"Процедура ЗаписатьОтказБанка(Знач ИмяСобытия)
	ЗаписьЖурналаРегистрации(ИмяСобытия,
		УровеньЖурналаРегистрации.Ошибка, , ,
		"Банк отклонил: " + ОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));
КонецПроцедуры
"#;
        check(code, expect![[r#""#]]);
    }

    // ── Внутри Исключение: уровень ────────────────────────────────────────

    #[test]
    fn test_wrong_log_level_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Информация, , Ссылка,\n\t\t\tПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:55
              message: Нужно указывать уровень "Ошибка" при записи в журнал регистрации внутри блока Исключение-КонецПопытки
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_dynamic_log_level_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньДляБанка(), , Ссылка,\n\t\t\tПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_variable_log_level_with_error_processing_detail() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖР, , Ссылка,\n\t\t\tОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(&code, expect![[r#""#]]);
    }

    // ── Внутри Исключение: комментарий ────────────────────────────────────

    #[test]
    fn test_correct_usage_in_except_with_detail() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_missing_detail_error_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tОписаниеОшибки());\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:21
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_correct_usage_in_except_with_raise() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tОписаниеОшибки());\n\t\tВызватьИсключение;\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_plain_string_comment_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Пакет не принят\");\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:22
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_concatenation_without_detail_error_in_except() {
        let code = in_except(
            "\t\tПричина = \"Код \" + Пакет.КодОтвета();\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина + \" / \" + Пакет.Статус());\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:37
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_unassigned_variable_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Отказ: \" + НеизвестнаяПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:35
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_variable_assigned_above_try_used_in_except() {
        let code = r#"Процедура ВыгрузитьВБанк(Пакет, Ссылка)
	Причина = "";
	Попытка
		Пакет.Отправить();
	Исключение
		ЗаписьЖурналаРегистрации("Банк.Выгрузка",
			УровеньЖурналаРегистрации.Ошибка, , Ссылка,
			Причина);
	КонецПопытки;
КонецПроцедуры
"#;
        check(code, expect![[r#""#]]);
    }

    #[test]
    fn test_variable_with_detail_error() {
        let code = in_except(
            "\t\tПричина = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_variable_traced_via_string_function_in_except() {
        let code = in_except(
            "\t\tПричина = СтрШаблон(\"Банк отклонил пакет: %1\",\n\t\t\tПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_variable_traced_via_concatenation_with_detail_error_in_except() {
        let code = in_except(
            "\t\tПричина =\n\t\t\t\"Пакет \" + Пакет.Номер\n\t\t\t\t+ Символы.ПС + ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_brief_error_used_directly_as_comment_in_except() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tКраткоеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:53
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_variable_traced_to_brief_error_in_except() {
        let code = in_except(
            "\t\tПричина = КраткоеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_two_variables_wrong_one_used_in_except() {
        let code = in_except(
            "\t\tКратко = КраткоеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tПодробно = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tКратко);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 7:3..8:11
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_brief_error_concatenated_with_description_in_except() {
        let code = in_except(
            "\t\tПричина = КраткоеПредставлениеОшибки(ИнформацияОбОшибке()) + ОписаниеОшибки();\n\t\tЗаписьЖурналаРегистрации(\n\t\t\t\"Банк.Выгрузка\",\n\t\t\tУровеньЖурналаРегистрации.Ошибка,\n\t\t\t,\n\t\t\tСсылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..11:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    // ── То же через ОбработкаОшибок ───────────────────────────────────────

    #[test]
    fn test_error_processing_module_variable_traced_to_detail() {
        let code = in_except(
            "\t\tПричина = ОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_error_processing_module_string_function_in_except() {
        let code = in_except(
            "\t\tПричина = СтрШаблон(\"Отказ банка: %1\",\n\t\t\tОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке()));\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_error_processing_module_concatenation_with_detail_in_except() {
        let code = in_except(
            "\t\tПричина = \"Пакет \" + Пакет.Номер\n\t\t\t+ Символы.ПС + ОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_error_processing_module_brief_used_directly() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tОбработкаОшибок.КраткоеПредставлениеОшибки(ИнформацияОбОшибке()));\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:69
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_error_processing_module_variable_traced_to_brief() {
        let code = in_except(
            "\t\tПричина = ОбработкаОшибок.КраткоеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_error_processing_module_two_variables_wrong_one_used() {
        let code = in_except(
            "\t\tКратко = ОбработкаОшибок.КраткоеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tПодробно = ОбработкаОшибок.ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tКратко);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 7:3..8:11
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_error_processing_module_brief_concatenated_with_description() {
        let code = in_except(
            "\t\tПричина = ОбработкаОшибок.КраткоеПредставлениеОшибки(ИнформацияОбОшибке()) + ОписаниеОшибки();\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    // ── Внутри Исключение: подробное представление через переменную ───────

    #[test]
    fn test_detail_of_error_info_saved_to_variable() {
        let code = in_except(
            "\t\tИнформация = ИнформацияОбОшибке();\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПодробноеПредставлениеОшибки(Информация));\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_detail_variable_concatenated_into_comment() {
        let code = in_except(
            "\t\tПодробно = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Пакет не отправлен:\" + Символы.ПС + Подробно);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_detail_saved_to_field() {
        let code = in_except(
            "\t\tПакет.ТекстОшибки = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Отказ банка: \" + Пакет.ТекстОшибки);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_detail_appended_to_variable_later() {
        let code = in_except(
            "\t\tТекст = \"Пакет не отправлен\";\n\t\tТекст = Текст + Символы.ПС + ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tТекст);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_detail_traced_through_two_variables() {
        let code = in_except(
            "\t\tИнформация = ИнформацияОбОшибке();\n\t\tПодробно = ПодробноеПредставлениеОшибки(Информация);\n\t\tСообщение = СтрШаблон(\"Пакет %1 не отправлен: %2\", Пакет.Номер, Подробно);\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tСообщение);\n",
        );
        check(&code, expect![[r#""#]]);
    }

    #[test]
    fn test_variable_with_unrelated_text_in_except() {
        let code = in_except(
            "\t\tПричина = \"Банк отклонил пакет\";\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_variable_overwritten_after_detail_in_except() {
        let code = in_except(
            "\t\tПричина = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tПричина = \"Банк отклонил пакет\";\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 7:3..8:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_brief_variable_concatenated_into_comment() {
        let code = in_except(
            "\t\tКратко = КраткоеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Отказ: \" + Кратко);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:23
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_of_unrelated_value() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПодробноеПредставлениеОшибки(Пакет.ПоследняяОшибка));\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:56
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_assigned_after_the_call() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Отказ: \" + Причина);\n\t\tПричина = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:24
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_name_inside_string_literal() {
        let code = in_except(
            "\t\tТекст = \"ПодробноеПредставлениеОшибки(Инфо)\";\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\t\"Отказ: \" + Текст);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 6:3..7:22
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_under_another_string_key() {
        let code = in_except(
            "\t\tСтруктура[\"Прочее\"] = \"Банк\";\n\t\tСтруктура[\"Текст\"] = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tСтруктура[\"Прочее\"]);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 7:3..8:24
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_under_the_same_string_key() {
        let code = in_except(
            "\t\tСтруктура[\"Текст\"] = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tСтруктура[\"Прочее\"] = \"Банк\";\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tСтруктура[\"Текст\"]);\n",
        );
        check(&code, expect![""]);
    }

    #[test]
    fn test_variable_assigned_only_after_the_call() {
        let code = in_except(
            "\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПричина);\n\t\tПричина = \"Банк отклонил пакет\";\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 5:3..6:12
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_detail_traced_through_a_long_chain_of_copies() {
        let code = in_except(
            "\t\tХ1 = ПодробноеПредставлениеОшибки(ИнформацияОбОшибке());\n\t\tХ2 = Х1;\n\t\tХ3 = Х2;\n\t\tХ4 = Х3;\n\t\tХ5 = Х4;\n\t\tХ6 = Х5;\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tХ6);\n",
        );
        check(&code, expect![""]);
    }

    #[test]
    fn test_error_info_copied_to_another_variable() {
        let code = in_except(
            "\t\tИнфо1 = ИнформацияОбОшибке();\n\t\tИнфо2 = Инфо1;\n\t\tПодробно = ПодробноеПредставлениеОшибки(Инфо2);\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tПодробно);\n",
        );
        check(&code, expect![""]);
    }

    #[test]
    fn test_detail_removed_by_extension() {
        let code = in_except(
            "\t\tТекст = \"Отказ: \" +\n\t\t#Удаление\n\t\tСимволы.ПС + ПодробноеПредставлениеОшибки(ИнформацияОбОшибке()) +\n\t\t#КонецУдаления\n\t\tОписаниеОшибки();\n\t\tЗаписьЖурналаРегистрации(\"Банк.Выгрузка\", УровеньЖурналаРегистрации.Ошибка, , Ссылка,\n\t\t\tТекст);\n",
        );
        check(
            &code,
            expect![[r#"
            UsageWriteLogEvent @ 10:3..11:10
              message: В тексте комментария нет вызова "ПодробноеПредставлениеОшибки(ИнформацияОбОшибке())"
              severity: Hint"#]],
        );
    }
}
