use crate::define_metadata;
use crate::metadata::*;
use crate::slab::{self, Block};
use crate::{AnalysisContext, Diagnostic, DiagnosticCode, DiagnosticsContext};
use hir::LocalRange;
use ide_db::TextRange;
use syntax::SyntaxKind;

pub const METADATA: DiagnosticMetadata = define_metadata! {
    diagnostic_type: DiagnosticType::CodeSmell,
    severity: DiagnosticSeverityLevel::Info,
    scope: DiagnosticScope::All,
    modules: &[],
    minutes_to_fix: 2,
    activated_by_default: true,
    compatibility_mode: DiagnosticCompatibilityMode::Undefined,
    tags: &[MetadataTag::Standard, MetadataTag::Badpractice],
    can_locate_on_project: false,
    extra_min_for_complexity: 0.0,
    lsp_severity_override: "",
    clean_code_attribute: CleanCodeAttribute::Consistent,
};

pub fn check(ctx: &DiagnosticsContext) -> Vec<Diagnostic> {
    slab::check_file_by_blocks(ctx, check_block)
}

pub fn check_block(ctx: &AnalysisContext, block: &Block) -> Vec<Diagnostic<LocalRange>> {
    let _span = tracing::debug_span!("IncorrectLineBreak::check").entered();
    let code = DiagnosticCode::IncorrectLineBreak;

    if ctx.is_disabled_with_metadata(code) {
        return Vec::new();
    }

    let text = block.text;
    let lines: Vec<&str> = line_index::split_lines(text).collect();

    let mut diagnostics = Vec::new();

    let mut line_tokens: Vec<Vec<(SyntaxKind, TextRange, &str)>> = vec![Vec::new(); lines.len()];

    for token in block.tokens {
        let kind = token.kind;

        if matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
            continue;
        }

        let range = token.range;
        let line = block.line_index.line_col(range.start()).line as usize;

        if line < lines.len() {
            line_tokens[line].push((kind, range, &text[range]));
        }
    }

    for (line_idx, tokens) in line_tokens.iter().enumerate() {
        if tokens.is_empty() {
            continue;
        }
        // За последней строкой блока продолжения литерала быть не может:
        // строку, начатую `"` или `|`, парсер держит в узле своего выражения,
        // и она лежит в том же блоке.
        // `\r\n`-половина `\r\r\n` — не строка модуля: платформа читает всю
        // последовательность как один перенос.
        let next_line = (line_idx + 1..lines.len())
            .find(|&next| !block.line_index.is_cr_crlf_tail(text, next as u32))
            .map(|next| lines[next]);
        if let Some(diag) = check_line_end(tokens, next_line, code, ctx) {
            diagnostics.push(diag);
        }
    }

    for tokens in line_tokens.iter() {
        if tokens.is_empty() {
            continue;
        }
        if let Some(diag) = check_line_start(tokens, code, ctx) {
            diagnostics.push(diag);
        }
    }

    diagnostics
}

fn is_forbidden_at_line_end(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::PLUS
            | SyntaxKind::MINUS
            | SyntaxKind::STAR
            | SyntaxKind::SLASH
            | SyntaxKind::PERCENT
            | SyntaxKind::KW_AND
            | SyntaxKind::KW_OR
    )
}

fn is_forbidden_at_line_start(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::R_PAREN | SyntaxKind::SEMICOLON)
}

fn check_line_end(
    tokens: &[(SyntaxKind, TextRange, &str)],
    next_line: Option<&str>,
    code: DiagnosticCode,
    ctx: &AnalysisContext,
) -> Option<Diagnostic<LocalRange>> {
    if let Some(next_line) = next_line {
        let trimmed = next_line.trim_start();
        if trimmed.starts_with('"') || trimmed.starts_with('|') {
            return None;
        }
    }

    let last_meaningful = tokens.iter().rev().find(|(kind, _, _)| *kind != SyntaxKind::COMMENT)?;

    let (kind, range, text) = last_meaningful;

    if is_forbidden_at_line_end(*kind) {
        return Some(Diagnostic {
            code,
            message: format!("Неправильный перенос строки: '{}' в конце строки", text.trim()),
            severity: ctx.severity(code),
            range: LocalRange::of_detached_node(*range),
            tags: ctx.tags(code),
            fixes: vec![],
        });
    }

    None
}

fn check_line_start(
    tokens: &[(SyntaxKind, TextRange, &str)],
    code: DiagnosticCode,
    ctx: &AnalysisContext,
) -> Option<Diagnostic<LocalRange>> {
    let (kind, range, text) = tokens.first()?;

    if is_forbidden_at_line_start(*kind) {
        return Some(Diagnostic {
            code,
            message: format!("Incorrect line break: '{}' at line start", text.trim()),
            severity: ctx.severity(code),
            range: LocalRange::of_detached_node(*range),
            tags: ctx.tags(code),
            fixes: vec![],
        });
    }

    if *kind == SyntaxKind::COMMA {
        let meaningful_after: Vec<_> =
            tokens.iter().skip(1).filter(|(k, _, _)| *k != SyntaxKind::COMMENT).collect();

        if !meaningful_after.is_empty() {
            let last_range = meaningful_after.last().map(|(_, r, _)| *r)?;
            let combined_range = TextRange::new(range.start(), last_range.end());
            return Some(Diagnostic {
                code,
                message: "Incorrect line break: ',' at line start".to_string(),
                severity: ctx.severity(code),
                range: LocalRange::of_detached_node(combined_range),
                tags: ctx.tags(code),
                fixes: vec![],
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::test_utils::{check_ast_diagnostic, format_diags};
    use expect_test::expect;

    fn snapshot(code: &str, expected: expect_test::Expect) {
        let diagnostics = check_ast_diagnostic(code, check);
        expected.assert_eq(&format_diags(code, &diagnostics));
    }

    #[test]
    fn test_correct_line_breaks() {
        let code = r#"Функция ВесПосылки(Посылка)
	Итог = Посылка.ВесТовара
		+ Посылка.ВесУпаковки
		- Посылка.Скидка;
	Если Итог > 30
		И Посылка.Хрупкая Тогда
		Итог = Итог * 2;
	КонецЕсли;
	Возврат Итог;
КонецФункции
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_operator_at_end_two_lines() {
        let code = r#"Итог = Посылка.ВесТовара +
	Посылка.ВесУпаковки -
	Посылка.Скидка;
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 1:26..1:27
              message: Неправильный перенос строки: '+' в конце строки
              severity: Hint
            IncorrectLineBreak @ 2:22..2:23
              message: Неправильный перенос строки: '-' в конце строки
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_operator_at_end_after_string_operand() {
        let code = r#"Заголовок = "Посылка № " +
	Посылка.Номер;
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 1:26..1:27
              message: Неправильный перенос строки: '+' в конце строки
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_operator_before_string_continuation_passes() {
        let code = r#"Текст = Текст +
"ВЫБРАТЬ
|	Посылки.Номер КАК Номер
|ИЗ
|	Документ.Посылка КАК Посылки";
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_multiline_string_ok() {
        let code = r#"Функция Подпись()
	Возврат "Отправитель: склад" +
		" № 4";
КонецФункции
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_logical_operators_at_line_end() {
        let code = r#"Процедура ПроверитьМаршрут(Посылка)
	Если Посылка.Срочная ИЛИ
		Посылка.Вес > 30 Тогда
		Посылка.Курьер = "Экспресс";
	КонецЕсли;
	Если (Посылка.Страна = "Казахстан") И
		(Посылка.Таможня = Неопределено) Тогда
		Посылка.Отложить();
	КонецЕсли;
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 2:23..2:26
              message: Неправильный перенос строки: 'ИЛИ' в конце строки
              severity: Hint
            IncorrectLineBreak @ 6:38..6:39
              message: Неправильный перенос строки: 'И' в конце строки
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_comma_at_line_start_with_content() {
        let code = r#"Маршрут.Добавить(Склады.Центральный.Адрес
	,Склады.Северный.Адрес);
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 2:2..2:26
              message: Incorrect line break: ',' at line start
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_lone_comma_placeholder_passes() {
        let code = r#"ЗаписатьСобытие(
	"Отгрузка",
	УровеньЖурналаРегистрации.Предупреждение,
	,
	ОписаниеОшибки);
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_closing_paren_at_line_start() {
        let code = r#"Функция Тариф(Посылка)
	Возврат РассчитатьТариф(Посылка.Вес, Посылка.Зона
	);
КонецФункции
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 3:2..3:3
              message: Incorrect line break: ')' at line start
              severity: Hint"#]],
        );
    }

    #[test]
    fn test_semicolon_at_line_start() {
        let code = r#"Процедура Отметить(Посылка)
	Посылка.Отправлена = Истина
	;
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            IncorrectLineBreak @ 3:2..3:3
              message: Incorrect line break: ';' at line start
              severity: Hint"#]],
        );
    }
}
