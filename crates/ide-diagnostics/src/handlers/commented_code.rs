use crate::define_metadata;
use crate::metadata::*;
use crate::slab::{self, Block};
use crate::{AnalysisContext, Diagnostic, DiagnosticCode, DiagnosticsContext, Fix, TextEdit};
use hir::LocalRange;
use ide_db::TextRange;
use lexer::{tokenize, Token, TokenKind};
use syntax::{CommentLine, CommentRun};

pub const METADATA: DiagnosticMetadata = define_metadata! {
    diagnostic_type: DiagnosticType::CodeSmell,
    severity: DiagnosticSeverityLevel::Minor,
    scope: DiagnosticScope::All,
    modules: &[],
    minutes_to_fix: 1,
    activated_by_default: true,
    compatibility_mode: DiagnosticCompatibilityMode::Undefined,
    tags: &[MetadataTag::Standard, MetadataTag::Badpractice],
    can_locate_on_project: false,
    extra_min_for_complexity: 0.0,
    lsp_severity_override: "",
};

/// `[diagnostics.parameters.CommentedCode] skipSupported`: stay silent in the
/// modules of objects the vendor still supports.
const SKIP_SUPPORTED: &str = "skipSupported";

#[derive(Debug, Clone)]
struct Config {
    exclusion_prefixes: Vec<String>,
    skip_supported: bool,
}

impl Config {
    fn from_context(ctx: &AnalysisContext) -> Self {
        let exclusion_prefixes_str =
            ctx.config_string(DiagnosticCode::CommentedCode, "exclusionPrefixes", "");

        let exclusion_prefixes: Vec<String> = exclusion_prefixes_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let skip_supported = ctx.config_bool(DiagnosticCode::CommentedCode, SKIP_SUPPORTED, false);

        Self { exclusion_prefixes, skip_supported }
    }
}

/// Whether the raw `[diagnostics]` section asks for the vendor support map: the
/// map is read from disk only for a project that uses it.
pub fn vendor_support_requested(diagnostics: &serde_json::Value) -> bool {
    diagnostics
        .get("parameters")
        .and_then(|parameters| parameters.get(DiagnosticCode::CommentedCode.as_str()))
        .and_then(|parameters| parameters.get(SKIP_SUPPORTED))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Whether this module belongs to an object the vendor still supports.
///
/// Commented-out code in a vendor module is how a vendor fragment is switched off
/// so that it survives the next update; the finding there cannot be acted on. The
/// owner is the metadata object the module belongs to: the form itself for a form
/// module (a form has its own support record, and a form added to a vendor object
/// is the developer's), the configuration root for the application, session and
/// external-connection modules. A module whose owner carries no uuid is not
/// treated as the vendor's.
fn module_is_vendor_supported(ctx: &AnalysisContext) -> bool {
    use bsl_metadata::{MdObject, ModuleType};

    let Some(support) = ctx.vendor_support() else { return false };
    let metadata = ctx.module_metadata();
    let owner = match metadata.module_type {
        ModuleType::ApplicationModule
        | ModuleType::ManagedApplicationModule
        | ModuleType::OrdinaryApplicationModule
        | ModuleType::SessionModule
        | ModuleType::ExternalConnectionModule => support.configuration_root(),
        _ => metadata
            .form
            .as_ref()
            .map(|form| form.uuid())
            .filter(|uuid| !uuid.is_nil())
            .or_else(|| metadata.mdo.as_ref().and_then(|mdo| mdo.uuid().copied()))
            .or_else(|| metadata.common_module.as_ref().map(|module| *module.uuid()))
            .or_else(|| metadata.register.as_ref().map(|register| *MdObject::uuid(&**register))),
    };
    owner.is_some_and(|uuid| support.is_supported(&uuid))
}

pub fn check(ctx: &DiagnosticsContext) -> Vec<Diagnostic> {
    slab::check_file_by_blocks(ctx, check_block)
}

pub fn check_block(ctx: &AnalysisContext, block: &Block) -> Vec<Diagnostic<LocalRange>> {
    let _span = tracing::debug_span!("CommentedCode::check").entered();
    let code = DiagnosticCode::CommentedCode;
    if ctx.is_disabled_with_metadata(code) {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    let config = Config::from_context(ctx);
    if config.skip_supported && module_is_vendor_supported(ctx) {
        return Vec::new();
    }
    let text = block.text;

    // Владения строкой диагностика не требует: комментарий за кодом — такой же
    // кандидат в закомментированный код, как стоящий на своей строке.
    for run in syntax::comment_runs_of(block.tokens) {
        if is_commented_code(&run, text, &config) {
            // Deleting commented-out code is destructive and easy to regret, so it is an
            // opt-in quick fix, never part of an unattended `source.fixAll` sweep. The whole
            // commented line(s) are removed, but only when they hold nothing but the
            // comments (no real code shares them — see `deletable_lines_range`).
            let fixes = deletable_lines_range(text, &run)
                .map(|delete_range| {
                    Fix::manual(
                        "Удалить закомментированный код",
                        vec![TextEdit {
                            range: LocalRange::of_detached_node(delete_range),
                            new_text: String::new(),
                        }],
                    )
                })
                .into_iter()
                .collect();
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::CommentedCode,
                message: message_ru(),
                range: LocalRange::of_detached_node(run.range()),
                severity: ctx.severity(code),
                tags: ctx.tags(code),
                fixes,
            });
        }
    }

    diagnostics
}

/// The full lines a comment group occupies — indentation and trailing newline included —
/// but only when the group is the sole content of those lines. Returns `None` if any real
/// code shares a line (before, between, or after the comments), so deleting never drops it.
fn deletable_lines_range(text: &str, run: &CommentRun) -> Option<TextRange> {
    let start: usize = run.range().start().into();
    let end: usize = run.range().end().into();
    let line_start = text[..start].rfind('\n').map_or(0, |nl| nl + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |nl| end + nl + 1);

    // Everything in the region that is not a comment token must be whitespace.
    let mut cursor = line_start;
    for line in run.lines() {
        let token_start: usize = line.range.start().into();
        if !text[cursor..token_start].trim().is_empty() {
            return None;
        }
        cursor = line.range.end().into();
    }
    if !text[cursor..line_end].trim().is_empty() {
        return None;
    }

    Some(TextRange::new((line_start as u32).into(), (line_end as u32).into()))
}

/// Strips the leading `//` markers from a single comment line's text.
fn comment_body<'a>(text: &'a str, line: &CommentLine) -> &'a str {
    text[line.range].trim_start_matches('/')
}

fn message_ru() -> String {
    "Программные модули не должны иметь закомментированных фрагментов кода".to_string()
}

/// Decides whether a group of consecutive `//` comments is commented-out BSL
/// code (as opposed to prose, structured documentation, or data).
///
/// A group is flagged when at least one of its lines has the syntactic shape of
/// a BSL statement and the group does not carry a recognised documentation
/// marker. Reporting the whole group range keeps a single finding aligned with
/// the block a developer would delete.
fn is_commented_code(run: &CommentRun, text: &str, config: &Config) -> bool {
    if run.lines().is_empty() {
        return false;
    }

    // Documentation blocks (`Параметры:`, `Возвращаемое значение:`, …) describe
    // an API; the structured parameter lines below such markers frequently end
    // in `;` and read like code without being executable.
    if is_documentation_block(run, text) {
        return false;
    }

    // A group whose first non-empty line is structured data (XML/HTML/JSON) and
    // where data lines form the majority is a commented-out data block, not
    // commented-out code.  The first-line guard prevents a group that opens with
    // a real assignment and continues with HTML fragments from being suppressed.
    if group_is_commented_data(run, text) {
        return false;
    }

    // The SQL text-block continuation prefix (`|`) only marks code when the same
    // group also opens a real query (`ВЫБРАТЬ`, `SELECT`, …); a lone `| прозаичный
    // хвост` is just a boxed comment, not a commented-out query.
    let group_has_query = run.lines().iter().any(|line| line_opens_query(comment_body(text, line)));

    run.lines().iter().any(|line| line_is_code(comment_body(text, line), config, group_has_query))
}

const DOC_MARKERS: &[&str] = &[
    "параметры:",
    "возвращаемое значение:",
    "возвращаемоезначение:",
    "пример:",
    "описание:",
    "parameters:",
    "returns:",
    "return value:",
    "example:",
    "description:",
];

fn is_documentation_block(run: &CommentRun, text: &str) -> bool {
    run.lines().iter().any(|line| {
        let lowered = comment_body(text, line).trim().to_lowercase();
        DOC_MARKERS.iter().any(|marker| lowered.starts_with(marker))
    })
}

/// True when a group is a commented-out data block (XML, HTML, JSON) rather
/// than commented-out code.  Two conditions must both hold:
///   1. The first non-empty comment line is itself data (so a group that opens
///      with a real statement and continues with HTML fragments is not skipped).
///   2. Data lines form a strict majority (> half) of all non-empty lines.
fn group_is_commented_data(run: &CommentRun, text: &str) -> bool {
    let bodies: Vec<&str> = run
        .lines()
        .iter()
        .map(|line| comment_body(text, line).trim())
        .filter(|s| !s.is_empty())
        .collect();

    if bodies.is_empty() {
        return false;
    }

    if !looks_like_commented_data(bodies[0]) {
        return false;
    }

    let data_count = bodies.iter().filter(|s| looks_like_commented_data(s)).count();
    if data_count * 2 <= bodies.len() {
        return false;
    }

    // A group that contains any line ending with `;` has at least one genuine
    // BSL statement in it, so it must not be suppressed wholesale.  Data
    // blocks (XML, HTML, JSON, BNF) never end lines with a semicolon.
    if bodies.iter().any(|s| s.ends_with(';')) {
        return false;
    }

    true
}

/// Classifies a single comment line (body already stripped of `//`) as having
/// the syntactic shape of a BSL statement.
fn line_is_code(body: &str, config: &Config, group_has_query: bool) -> bool {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return false;
    }

    for prefix in &config.exclusion_prefixes {
        if trimmed.starts_with(prefix) {
            return false;
        }
    }

    let lowered = trimmed.to_lowercase();
    if DOC_MARKERS.iter().any(|marker| lowered.starts_with(marker)) {
        return false;
    }

    // High-precision prose veto: natural-language text routinely places two bare
    // nouns next to each other ("тип свойства", "выбора группы"), which real BSL
    // never does — there is always a `.`, `(`, `,`, `=`, or operator between two
    // names. This runs before any keyword signal so a sentence that merely starts
    // with `Если`/`Попытка`/`Возврат` cannot be mistaken for a statement.
    //
    // Tokenize once here and thread the slice into the downstream helpers so the
    // lexer is not invoked multiple times for the same text.
    let toks = code_tokens(trimmed);
    if has_two_adjacent_idents(&toks) {
        return false;
    }

    // Commented-out data (JSON objects, HTML/markup) is not commented-out code.
    // Keyed on markup/JSON shape, not on the bare `<`/`>` characters, so a real
    // comparison such as `ТипЗнч(...) <> Тип(...)` is still recognised as code.
    if looks_like_commented_data(trimmed) {
        return false;
    }

    strong_statement(trimmed, &toks, group_has_query)
}

/// True when two consecutive non-trivia tokens are both bare identifiers (each is
/// `Ident`, i.e. neither a keyword nor punctuation/operator/number/string). This
/// is the natural-language prose signature.
fn has_two_adjacent_idents(toks: &[Token]) -> bool {
    toks.windows(2).any(|w| w[0].kind == TokenKind::Ident && w[1].kind == TokenKind::Ident)
}

/// True when the (uncommented) line is data rather than BSL: a JSON object/array
/// fragment or an HTML/XML/markup tag. Conservative on purpose — it keys on
/// markup shape so BSL using `<`/`>` as comparison operators is never suppressed.
fn looks_like_commented_data(trimmed: &str) -> bool {
    let body = trimmed.trim_start_matches('|').trim_start();
    let first = body.chars().next();

    // JSON object/array braces, or a `"key": value` pair.
    if matches!(first, Some('{') | Some('}')) {
        return true;
    }
    if is_json_pair(body) {
        return true;
    }

    // Opening/closing markup tag: `<tag`, `</tag`, `<?xml`, `<!DOCTYPE`,
    // `<Формула>` (Cyrillic).  A BSL statement never begins with `<`, so any
    // `<` followed by `/`, `?`, `!`, or any Unicode letter is data.
    if first == Some('<') {
        let after = body[1..].trim_start();
        let name = after.trim_start_matches('/');
        if let Some(c) = name.chars().next() {
            if c == '?' || c == '!' || c.is_alphabetic() {
                return true;
            }
        }
    }

    false
}

/// Recognises a JSON member line of the form `"name": value`.
fn is_json_pair(body: &str) -> bool {
    let bytes = body.as_bytes();
    if bytes.first() != Some(&b'"') {
        return false;
    }
    let mut idx = 1;
    while idx < bytes.len() && bytes[idx] != b'"' {
        idx += 1;
    }
    if idx >= bytes.len() {
        return false;
    }
    body[idx + 1..].trim_start().starts_with(':')
}

const QUERY_KW: &[&str] = &[
    "выбрать",
    "select",
    "из",
    "from",
    "где",
    "where",
    "сгруппировать",
    "group",
    "упорядочить",
    "order",
    "объединить",
    "union",
    "имеющие",
    "having",
];

/// True when a `|`-prefixed text-block line opens a BSL query (`ВЫБРАТЬ`, …),
/// marking the surrounding block as a commented-out query rather than a boxed
/// prose comment.
fn line_opens_query(body: &str) -> bool {
    let trimmed = body.trim();
    let Some(rest) = trimmed.strip_prefix('|') else {
        return false;
    };
    let lowered = rest.trim_start().to_lowercase();
    QUERY_KW.iter().any(|kw| {
        lowered
            .strip_prefix(kw)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(|c: char| c.is_whitespace()))
    })
}

/// Returns the BSL tokens of a comment line, minus whitespace.
///
/// The set is whitespace alone, deliberately, and not `TokenKind::is_trivia`:
/// what is being tokenized is the body of a comment, so a second `//` in it
/// is code commented twice — `// //Модуль.Метод(Данные);` — and it swallows
/// the rest of the line into one `Comment` token. Dropping that token leaves
/// the line with nothing to judge, and the check stops seeing exactly the
/// thing it looks for. Measured on real modules: of 96 comment lines carrying
/// a second `//`, widening the set loses that one verdict and changes no
/// other.
fn code_tokens(text: &str) -> Vec<Token> {
    tokenize(text).into_iter().filter(|t| t.kind != TokenKind::Whitespace).collect()
}

fn ends_with(text: &str, ch: char) -> bool {
    text.trim_end().ends_with(ch)
}

/// A bare member reference (`Имя`, `Модуль.Метод`, `Объект.Метод()`) is a name
/// mention in prose, not an executable statement.
fn is_bare_reference(toks: &[Token]) -> bool {
    if toks.is_empty() {
        return false;
    }
    let mut i = 0;
    if toks[i].kind != TokenKind::Ident {
        return false;
    }
    i += 1;
    while i + 1 < toks.len()
        && toks[i].kind == TokenKind::Dot
        && toks[i + 1].kind == TokenKind::Ident
    {
        i += 2;
    }
    if i + 1 < toks.len()
        && toks[i].kind == TokenKind::LParen
        && toks[i + 1].kind == TokenKind::RParen
    {
        i += 2;
    }
    if i < toks.len() && toks[i].kind == TokenKind::Dot {
        i += 1;
    }
    i == toks.len()
}

/// Detects `Идент … = …` assignment, returning the index of the `=` token.
/// Rejects comparison (`<=`, `>=`, `<>`, `==`) and arrow (`=>`) shapes.
/// The token before `=` must be an identifier, `)`, or `]` — a numeric literal
/// cannot be an assignment target in BSL.
fn assignment_index(toks: &[Token]) -> Option<usize> {
    for idx in 1..toks.len().saturating_sub(1) {
        if toks[idx].kind != TokenKind::Eq {
            continue;
        }
        let prev = toks[idx - 1].kind;
        let next = toks[idx + 1].kind;
        if matches!(prev, TokenKind::Lt | TokenKind::Gt | TokenKind::Eq) {
            continue;
        }
        if matches!(next, TokenKind::Eq | TokenKind::Gt) {
            continue;
        }
        if matches!(prev, TokenKind::Ident | TokenKind::RParen | TokenKind::RBracket) {
            return Some(idx);
        }
    }
    None
}

/// `Идент . Идент (` — a qualified method call.
fn has_member_call(toks: &[Token]) -> bool {
    toks.windows(4).any(|w| {
        w[0].kind == TokenKind::Ident
            && w[1].kind == TokenKind::Dot
            && w[2].kind == TokenKind::Ident
            && w[3].kind == TokenKind::LParen
    })
}

const DECL_KW: &[TokenKind] = &[TokenKind::KwProcedure, TokenKind::KwFunction];
const END_KW: &[TokenKind] = &[
    TokenKind::KwEndProcedure,
    TokenKind::KwEndFunction,
    TokenKind::KwEndIf,
    TokenKind::KwEndDo,
    TokenKind::KwEndTry,
];
const SIMPLE_STMT_KW: &[TokenKind] = &[
    TokenKind::KwReturn,
    TokenKind::KwContinue,
    TokenKind::KwBreak,
    TokenKind::KwGoto,
    TokenKind::KwTry,
    TokenKind::KwExcept,
    TokenKind::KwVar,
    TokenKind::KwRaise,
];
const COND_KW: &[TokenKind] = &[TokenKind::KwIf, TokenKind::KwFor, TokenKind::KwWhile];
const PAIR_KW: &[TokenKind] = &[TokenKind::KwThen, TokenKind::KwDo];

fn has_code_operator(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '=' | '+' | '*' | '/'))
}

/// True when a line carries an unambiguous statement structure: an assignment,
/// a declaration with parameters, a block keyword, a paired conditional/loop, a
/// terminated call, or a query-language continuation.
///
/// `toks` is the pre-computed non-whitespace token slice for `trimmed`,
/// produced once by `line_is_code` to avoid redundant lexer invocations.
fn strong_statement(trimmed: &str, toks: &[Token], group_has_query: bool) -> bool {
    // SQL/query string continuation lines begin with the `|` text-block prefix,
    // but only count when the surrounding block actually opens a query — a lone
    // `| прозаичный хвост` is a boxed comment, not commented-out SQL.
    if trimmed.starts_with('|') {
        return group_has_query;
    }

    if toks.is_empty() {
        return false;
    }

    // A trailing `Dot` token is sentence punctuation, not member-access syntax:
    // real member access always has an identifier after the dot, so a dot that
    // is the last token cannot be part of an expression.  Strip it before the
    // structural tests so `М = Менеджер.` is not treated as an assignment whose
    // RHS contains a Dot (which would otherwise trigger `rhs_is_code`).
    let toks = if toks.last().map(|t| t.kind) == Some(TokenKind::Dot) {
        &toks[..toks.len() - 1]
    } else {
        toks
    };

    // The Dot strip above can leave toks empty (e.g. a comment body of just
    // `.` or `..`).  Nothing to classify — not code.
    if toks.is_empty() {
        return false;
    }

    if is_bare_reference(toks) {
        return false;
    }

    let ends_period = ends_with(trimmed, '.');
    let ends_semi = ends_with(trimmed, ';');
    let opens = trimmed.matches('(').count();
    let closes = trimmed.matches(')').count();
    let has_string = toks.iter().any(|t| t.kind == TokenKind::String);

    if let Some(idx) = assignment_index(toks) {
        let rhs = &toks[idx + 1..];
        let rhs_is_code = rhs.iter().any(|t| {
            matches!(
                t.kind,
                TokenKind::Decimal
                    | TokenKind::Float
                    | TokenKind::String
                    | TokenKind::LParen
                    | TokenKind::Dot
                    | TokenKind::Plus
                    | TokenKind::Star
                    | TokenKind::Slash
                    | TokenKind::Semicolon
                    | TokenKind::RParen
            ) || t.kind.is_keyword()
        });
        if ends_semi || rhs_is_code || opens > 0 {
            return true;
        }
        // A bare `X = Y` where the RHS is a lone identifier is ambiguous: it
        // could be genuine commented code or a prose equation ("Параметр =
        // Прочтена"). Reject it unless it ends with `;`, has code in the RHS,
        // or has open parens — all of which are handled above. A period-
        // terminated line is already excluded by `ends_period`; non-period bare
        // assignments also lack enough signal to distinguish code from prose.
        return false;
    }

    let first = toks[0].kind;
    if DECL_KW.contains(&first) {
        return opens > 0;
    }
    if END_KW.contains(&first) {
        return true;
    }
    if SIMPLE_STMT_KW.contains(&first) {
        // `Попытка`/`Исключение` as standalone keywords (`toks.len() == 1`) are
        // unambiguous block delimiters; with following words they could be prose
        // ("Попытка №1.", "Исключение для пользователя.") so they fall through
        // to the shared gate below like all other simple statement keywords.
        if ends_semi || opens > 0 || has_code_operator(trimmed) {
            return true;
        }
        if toks.len() == 1 {
            return true;
        }
    }

    if ends_semi && opens > 0 {
        return true;
    }

    // `Идент.Метод(` is a call statement, but `Если Х Тогда`-style conditionals
    // also appear verbatim in prose ("Если …, тогда …"); guard those soft
    // signals so a full sentence ending in a period is never treated as code.
    let prose = ends_period || is_prose(trimmed, toks);

    if has_member_call(toks) && !prose {
        return true;
    }

    let has_pair = toks.iter().any(|t| PAIR_KW.contains(&t.kind));
    let has_cond = toks.iter().any(|t| COND_KW.contains(&t.kind));
    if has_pair && has_cond && !prose {
        return true;
    }

    if opens > 0 && !prose {
        let comma_in_parens = comma_inside_parens(trimmed);
        if (comma_in_parens || has_string) && (ends_semi || opens > closes) {
            return true;
        }
    }

    false
}

fn comma_inside_parens(text: &str) -> bool {
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth > 0 => return true,
            _ => {}
        }
    }
    false
}

/// True when a line reads like natural-language prose: a run of mostly Cyrillic
/// words with almost no code operators. A member call or a real operator-bearing
/// expression disqualifies prose, but an isolated quoted word inside a sentence
/// (`… реквизит "Партнер" …`) does not.
///
/// `toks` is the pre-computed non-whitespace token slice for `trimmed`, reused
/// from `line_is_code` to avoid a redundant lexer call.
fn is_prose(trimmed: &str, toks: &[Token]) -> bool {
    let body = trimmed.trim_start_matches(['+', '-']).trim_start();
    // When the leading markers were stripped the token slice may cover more
    // text than `body`; re-tokenize only the stripped body for the word counts
    // so that a leading `+` operator is not mistaken for a code operator.
    let body_toks = if body.len() == trimmed.len() {
        // no leading markers — reuse the slice already computed
        std::borrow::Cow::Borrowed(toks)
    } else {
        std::borrow::Cow::Owned(code_tokens(body))
    };
    if has_member_call(&body_toks) {
        return false;
    }
    let words =
        body_toks.iter().filter(|t| t.kind == TokenKind::Ident || t.kind.is_keyword()).count();
    let cyrillic_words = body_toks
        .iter()
        .filter(|t| {
            (t.kind == TokenKind::Ident || t.kind.is_keyword())
                && t.text.chars().next().is_some_and(is_cyrillic)
        })
        .count();
    let code_ops = body
        .chars()
        .filter(|c| matches!(c, '=' | '(' | ')' | '[' | ']' | ';' | '+' | '*' | '/' | '%'))
        .count();
    words >= 4 && code_ops < 2 && cyrillic_words >= 3
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

#[cfg(test)]
mod tests {
    use super::check;
    use crate::test_utils::{check_ast_diagnostic, check_fix_snapshot_for, format_diags};
    use crate::DiagnosticCode;
    use expect_test::expect;

    fn snapshot(code: &str, expected: expect_test::Expect) {
        let diagnostics = check_ast_diagnostic(code, check);
        expected.assert_eq(&format_diags(code, &diagnostics));
    }

    #[test]
    fn test_fix_deletes_commented_code_manual() {
        let code =
            "Функция УровеньВоды(Бак)\n\t//Бак.Долить(5);\n\tВозврат Бак.Уровень;\nКонецФункции\n";
        check_fix_snapshot_for(
            code,
            DiagnosticCode::CommentedCode,
            expect![[r#"
            CommentedCode @ 2:2..2:18 — Удалить закомментированный код [fix_all=false]
            Функция УровеньВоды(Бак)
            	Возврат Бак.Уровень;
            КонецФункции
        "#]],
        );
    }

    #[test]
    fn commented_code_inside_multiline_literal_is_not_reported() {
        // Строки многострочного литерала, начинающиеся с `//`, лексер отдаёт
        // токенами комментария — текстом запроса они от этого быть не перестают.
        let inside_literal = "Процедура ПодготовитьЗапрос()\n\tТекст = \"ВЫБРАТЬ Грядки.Номер\n\t// Полить(Грядка);\n\t// Прервать;\n\t|ИЗ Справочник.Грядки КАК Грядки\";\nКонецПроцедуры\n";
        assert!(check_ast_diagnostic(inside_literal, check).is_empty());

        // Оборванный литерал, дотянувшийся до конца тела, — тот же текст строки.
        let unclosed_literal = "Процедура ПодготовитьЗапрос()\n\tТекст = \"ВЫБРАТЬ Грядки.Номер\n\t// Полить(Грядка);\n\t// Прервать;\nКонецПроцедуры\n";
        assert!(
            check_ast_diagnostic(unclosed_literal, check).is_empty(),
            "оборванный литерал до конца тела — текст строки, а не находка"
        );

        // Контроль: значимый токен до конца тела — строка оборвана, находка остаётся.
        let interrupted_tail = "Процедура ПодготовитьЗапрос()\n\tТекст = \"не закрыта\n\t// Полить(Грядка);\n\tНомер = 2;\nКонецПроцедуры\n";
        assert_eq!(
            check_ast_diagnostic(interrupted_tail, check).len(),
            1,
            "оборванная строка со значимым токеном впереди не глушит находку"
        );

        // Тот же текст вне литерала: без него пустой ответ выше означал бы и
        // «литерал распознан», и «диагностика перестала срабатывать вовсе».
        let outside_literal =
            "Процедура ПодготовитьЗапрос()\n\t// Полить(Грядка);\n\t// Прервать;\nКонецПроцедуры\n";
        snapshot(
            outside_literal,
            expect![[r#"
            CommentedCode @ 2:2..3:14
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn inline_code_comment_is_still_reported() {
        // Комментарий за кодом остаётся кандидатом в закомментированный код:
        // отсутствие быстрой правки не означает отсутствия находки.
        let code = "Функция Влажность(Датчик)\n\tЗначение = Датчик.Прочитать(); // Значение = 40;\n\tВозврат Значение;\nКонецФункции\n";
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:33..2:50
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_fix_not_offered_for_inline_code_comment() {
        // The comment body looks like code, but real code precedes it on the line, so
        // deleting the line would drop the assignment — no fix is offered.
        let code = "Функция Влажность(Датчик)\n\tЗначение = Датчик.Прочитать(); // Значение = 40;\n\tВозврат Значение;\nКонецФункции\n";
        check_fix_snapshot_for(code, DiagnosticCode::CommentedCode, expect![]);
    }

    #[test]
    fn test_no_diagnostic_for_regular_comments() {
        let code = r#"Функция НормаПолива(Культура)
	// Норма зависит от культуры и сезона
	// Значения взяты из агрономического справочника
	Литров = Культура.Норма;
	Возврат Литров;
КонецФункции
"#;
        snapshot(code, expect![[r#""#]]);
    }

    /// Код, закомментированный дважды, остаётся находкой: тело комментария
    /// само начинается с `//`, и предикат тривии не должен унести его вместе
    /// с находкой.
    #[test]
    fn code_commented_twice_is_still_a_finding() {
        let code = "Функция НормаПолива(Культура)\n\t// //Полив.Запустить(Грядка, Объем);\n\tВозврат Культура.Норма;\nКонецФункции\n";
        let diagnostics = check_ast_diagnostic(code, check);
        assert_eq!(diagnostics.len(), 1, "дважды закомментированный код перестал быть находкой");
    }

    /// BOM в начале файла не меняет вердикт.
    #[test]
    fn a_byte_order_mark_does_not_change_the_verdict() {
        let with_bom = "\u{feff}Процедура Проветрить()\n\t// Форточка.Открыть();\nКонецПроцедуры\n";
        let without = "Процедура Проветрить()\n\t// Форточка.Открыть();\nКонецПроцедуры\n";
        assert_eq!(
            check_ast_diagnostic(with_bom, check).len(),
            check_ast_diagnostic(without, check).len(),
            "BOM изменил число находок"
        );
        assert_eq!(check_ast_diagnostic(without, check).len(), 1, "вход обязан давать находку");
    }

    #[test]
    fn test_commented_assignment() {
        let code = r#"Функция Температура(Термометр)
	Градусы = Термометр.Показание;
	// Градусы = Градусы - 2;
	Возврат Градусы;
КонецФункции
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 3:2..3:27
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_multiline_commented_block() {
        let code = r#"//ПоливВключен = Истина;
//Пока Бак.Уровень < 10 Цикл
//	Бак.Долить(1);
//КонецЦикла;
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 1:1..4:14
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_commented_out_procedure() {
        let code = r#"////Процедура ОбновитьГрафик()
////
////	СдвинутьПолив(1);
////
////КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 1:1..5:19
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_two_consecutive_commented_lines() {
        let code = r#"//Журнал.Добавить("Полив", ТекущаяДатаСеанса());
//Журнал.Добавить("Проветривание", ТекущаяДатаСеанса());
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 1:1..2:57
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_range_covers_whole_group_with_header() {
        let code = r#"Процедура Сверить()
	// ++ временно отключено
	//Остаток = Склад.Остаток("Семена");
	//Пока Остаток > 0 Цикл
	//	Остаток = Остаток - 1;
	//КонецЦикла;
	// -- конец отключения
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..7:24
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_method_documentation_not_flagged() {
        let code = r#"// Рассчитывает объём полива на сутки.
//
// Параметры:
//  Грядка - СправочникСсылка.Грядки - участок полива;
//
// Возвращаемое значение:
//  Число - объём в литрах.
Функция СуточныйОбъем(Грядка)
	Возврат 0;
КонецФункции
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_header_attached_to_method_is_flagged() {
        // A commented-out assignment directly above a method declaration is
        // genuine commented code and must be flagged regardless of proximity.
        let code = r#"// Автополив = Ложь;
&НаКлиенте
Процедура ПереключитьРежим()
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 1:1..1:21
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_prose_not_flagged() {
        let code = r#"Процедура Проветрить()
	// Если в теплице жарко, открываем обе форточки сразу, а не по одной.
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_marker_comment_not_flagged() {
        let code = r#"Процедура Проветрить()
	// +Теплица2
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_method_reference_not_flagged() {
        let code = r#"Процедура Проветрить()
	// ПересчитатьГрафикПолива()
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_floating_commented_procedure_flagged() {
        let code = r#"Процедура Действующая()
	//Процедура Прежняя()
	//	СдвинутьПолив(2);
	//КонецПроцедуры
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..4:18
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_prose_with_leading_keyword_not_flagged() {
        let code = r#"Процедура Проветрить()
	// Попытка открыть форточку повторяется трижды.
	// Исключение делаем для зимнего режима теплицы.
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_member_assignment_flagged() {
        let code = r#"Процедура Проветрить()
	// Форточка.Угол = 30;
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..2:24
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_real_conditional_still_flagged() {
        let code = r#"Процедура Проветрить()
	// Если Датчик.Показание("Температура") > 28 Тогда
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..2:52
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_json_data_not_flagged() {
        let code = r#"Процедура Проветрить()
	// { "zone": "north" }
	// "angle": 30,
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_html_markup_not_flagged() {
        let code = r#"Процедура Проветрить()
	// <span class="warn">Жарко</span>
	// <table border="1">
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_comparison_operator_not_treated_as_markup() {
        let code = r#"Процедура Проветрить()
	// Если Режим <> "Зима" Тогда
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..2:31
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_lone_bar_comment_not_flagged() {
        let code = r#"Процедура Проветрить()
	// | раздел режимов проветривания
	// | см. также график полива
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_bar_query_block_flagged() {
        let code = r#"Процедура Проветрить()
	// |ВЫБРАТЬ
	// |	Форточки.Угол КАК Угол
	// |ИЗ
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..4:8
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_trailing_plus_marker_not_flagged() {
        let code = r#"Процедура Проветрить()
	// + 2 дня
	// ++ Доработка зимнего режима
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    /// A numeric literal cannot be an assignment target.
    #[test]
    fn test_number_lhs_not_flagged() {
        let code = r#"Процедура Проветрить()
	// 86400 = 24 * 60 * 60.
	// 2*1000 = 2 л
	// 7%2=1, 8%2=0
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    /// Попытка/Исключение followed by more words is prose.
    #[test]
    fn test_try_except_prose_not_flagged() {
        let code = r#"Процедура Проветрить()
	// Попытка вторая.
	// Исключение для северной стены.
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_bare_try_still_flagged() {
        let code = r#"Процедура Проветрить()
	// Попытка
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..2:12
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    /// XML, SOAP and BNF blocks are commented data, not code.
    #[test]
    fn test_xml_block_not_flagged() {
        let code = r#"Процедура Проветрить()
	// <?xml version="1.0" encoding="UTF-8"?>
	// <climate xmlns="urn:greenhouse:climate">
	//   <zone name="north"/>
	// </climate>
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_cyrillic_tag_not_flagged() {
        let code = r#"Процедура Проветрить()
	// <Режим> ::= <Сезон> "/" <Зона>
	// <Зона> ::= север | юг
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_code_before_html_still_flagged() {
        // Group opens with a real assignment — must not be suppressed by the
        // data-majority guard even though continuation lines are HTML.
        let code = r#"Процедура Проветрить()
	//Отчет = Отчет + "
	// |<B>Север</B>
	// |<BR>
	//|<I>закрыто</I>";
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..5:21
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    /// A trailing sentence dot is not member access.
    #[test]
    fn test_trailing_dot_legend_not_flagged() {
        let code = r#"Процедура Проветрить()
	// Т = Температура.
КонецПроцедуры
"#;
        snapshot(code, expect![[r#""#]]);
    }

    #[test]
    fn test_real_member_assign_still_flagged() {
        // The dot here is NOT final — it separates the object and its attribute — so
        // the trailing-dot strip must not remove it.
        let code = r#"Процедура Проветрить()
	// Угол = Форточка.Угол;
КонецПроцедуры
"#;
        snapshot(
            code,
            expect![[r#"
            CommentedCode @ 2:2..2:26
              message: Программные модули не должны иметь закомментированных фрагментов кода
              severity: Information"#]],
        );
    }

    #[test]
    fn test_exclusion_prefix() {
        let code = r#"Процедура ПримерВызова()
	//!! Пока Бак.Пуст Цикл
	//!! 	Бак.Долить(1);
	//!! КонецЦикла;
КонецПроцедуры
"#;
        let mut config = crate::DiagnosticsConfig::all_enabled();
        config
            .parameters
            .insert(DiagnosticCode::CommentedCode, serde_json::json!({"exclusionPrefixes": "!!"}));
        let diagnostics = crate::test_utils::check_ast_diagnostic_with_config(code, config, check);
        expect![[r#""#]].assert_eq(&format_diags(code, &diagnostics));
    }

    #[test]
    fn test_dot_only_comment_not_flagged() {
        // A comment body of just `.` or `..` must not panic and must not be flagged.
        snapshot("Процедура Пауза()\n\t// ..\n\t// .\nКонецПроцедуры\n", expect![[r#""#]]);
    }

    /// `!=` в теле комментария — сравнение, а не присваивание.
    ///
    /// Пара входов обязательна: на одном лишь `!=` проверка зелена при любой
    /// реализации `assignment_index`, включая ту, что не находит присваиваний
    /// вовсе. Присваивания рядом показывают, что мерка вообще способна их видеть.
    #[test]
    fn bang_equals_is_not_an_assignment() {
        let index = |text: &str| super::assignment_index(&super::code_tokens(text));

        assert_eq!(index("угол != 30"), None);
        assert_eq!(index("Зоны[2] != 30"), None);
        assert_eq!(index("Угол() != 30"), None);

        assert!(
            index("угол = 30;").is_some(),
            "мерка не видит присваивания и потому ничего не пиннит"
        );
        assert!(index("Зоны[2] = 30;").is_some());
        assert!(index("Угол() = 30;").is_some());
    }

    #[test]
    fn test_data_group_with_statement_still_flagged() {
        // A group that looks like data (XML tags) but contains a genuine BSL
        // statement ending in `;` must not be suppressed by group_is_commented_data.
        let code = "Процедура Проветрить()\n\t// <zone>\n\t// Форточка.Закрыть();\n\t// </zone>\nКонецПроцедуры\n";
        let diagnostics = crate::test_utils::check_ast_diagnostic(code, check);
        assert!(
            !diagnostics.is_empty(),
            "expected CommentedCode to fire when a group contains a BSL statement"
        );
    }
}

/// `skipSupported`: the modules of vendor objects are silent, the developer's own
/// modules keep the finding, and nothing changes while the setting is off.
#[cfg(test)]
mod vendor_support_tests {
    use super::{check, vendor_support_requested};
    use crate::test_utils::{check_metadata_diagnostic_with_vendor_support, format_diags};
    use crate::{DiagnosticCode, DiagnosticsConfig};
    use bsl_metadata::{Form, FormType, MdoType, MetadataObject, ModuleType, Uuid, VendorSupport};

    const ROOT: &str = "5e0d0000-0000-4000-8000-000000000001";
    const VENDOR_DOCUMENT: &str = "5e0d0000-0000-4000-8000-000000000002";
    const VENDOR_FORM: &str = "5e0d0000-0000-4000-8000-000000000003";
    const REMOVED_DOCUMENT: &str = "5e0d0000-0000-4000-8000-000000000004";
    const OWN_DOCUMENT: &str = "5e0d0000-0000-4000-8000-000000000101";
    const OWN_FORM: &str = "5e0d0000-0000-4000-8000-000000000102";

    const CODE: &str = "Процедура ПередЗаписью(Отказ)\n\t// Если Отказ Тогда\n\t// \tВозврат;\n\t// КонецЕсли;\nКонецПроцедуры\n";

    /// The map in the layout the platform writes: records `mode, flag, uuid, uuid`.
    fn support() -> VendorSupport {
        let records = [(1, ROOT), (1, VENDOR_DOCUMENT), (2, VENDOR_FORM), (0, REMOVED_DOCUMENT)]
            .iter()
            .map(|(mode, uuid)| format!(",{mode},0,{uuid},{uuid}"))
            .collect::<String>();
        let text = format!(
            "\u{feff}{{6,0,1,5e0d0000-0000-4000-8000-0000000000a1,0,\
             5e0d0000-0000-4000-8000-0000000000a2,\"1.0.0.1\",\"Поставщик1\",\"Конфигурация1\",4{records},\
             0,0,0,1,0,0,0,1,0,1,0,1,1,1,1}}"
        );
        VendorSupport::parse(text.as_bytes()).unwrap().with_configuration_root(uuid(ROOT))
    }

    fn uuid(text: &str) -> Uuid {
        Uuid::parse_str(text).unwrap()
    }

    fn document(owner: &str, module_type: ModuleType, form: Option<&str>) -> hir::ModuleMetadata {
        let mut mdo = MetadataObject::new(MdoType::Document, "Документ1");
        mdo.set_uuid(uuid(owner));
        let mut metadata = crate::test_utils::make_non_common_module_metadata(module_type);
        metadata.mdo = Some(std::sync::Arc::new(mdo));
        metadata.form = form.map(|form| {
            std::sync::Arc::new(Form::new(
                "ФормаДокумента".to_owned(),
                FormType::Managed,
                uuid(form),
            ))
        });
        metadata
    }

    fn config(skip_supported: Option<bool>) -> DiagnosticsConfig {
        let mut config = DiagnosticsConfig::all_enabled();
        if let Some(value) = skip_supported {
            config
                .parameters
                .insert(DiagnosticCode::CommentedCode, serde_json::json!({"skipSupported": value}));
        }
        config
    }

    fn run(metadata: hir::ModuleMetadata, skip_supported: Option<bool>) -> String {
        let diagnostics = check_metadata_diagnostic_with_vendor_support(
            metadata,
            CODE,
            config(skip_supported),
            Some(support()),
            check,
        );
        format_diags(CODE, &diagnostics)
    }

    const REPORTED: &str = "CommentedCode @ 2:2..4:15\n  message: Программные модули не должны иметь закомментированных фрагментов кода\n  severity: Information";

    #[test]
    fn vendor_object_module_is_silent_with_the_setting_on() {
        assert_eq!(run(document(VENDOR_DOCUMENT, ModuleType::ObjectModule, None), Some(true)), "");
    }

    #[test]
    fn setting_off_or_absent_leaves_vendor_modules_reported() {
        for setting in [None, Some(false)] {
            assert_eq!(
                run(document(VENDOR_DOCUMENT, ModuleType::ObjectModule, None), setting),
                REPORTED,
                "{setting:?}"
            );
        }
    }

    #[test]
    fn own_object_module_is_still_reported() {
        assert_eq!(
            run(document(OWN_DOCUMENT, ModuleType::ObjectModule, None), Some(true)),
            REPORTED
        );
    }

    #[test]
    fn object_removed_from_support_is_reported() {
        assert_eq!(
            run(document(REMOVED_DOCUMENT, ModuleType::ObjectModule, None), Some(true)),
            REPORTED
        );
    }

    /// A form has its own support record: the vendor's form of a vendor object is
    /// silent, a form the developer added to it is not.
    #[test]
    fn form_module_is_judged_by_the_form_itself() {
        let vendor_form = document(VENDOR_DOCUMENT, ModuleType::FormModule, Some(VENDOR_FORM));
        assert_eq!(run(vendor_form, Some(true)), "");
        let own_form = document(VENDOR_DOCUMENT, ModuleType::FormModule, Some(OWN_FORM));
        assert_eq!(run(own_form, Some(true)), REPORTED);
    }

    /// The application, session and external-connection modules belong to the
    /// configuration root, whose record the map carries like any object's.
    #[test]
    fn application_module_is_judged_by_the_configuration_root() {
        let metadata = crate::test_utils::make_non_common_module_metadata(
            ModuleType::ManagedApplicationModule,
        );
        assert_eq!(run(metadata.clone(), Some(true)), "");
        assert_eq!(run(metadata, Some(false)), REPORTED);
    }

    /// No map (the configuration was never on support): the setting changes nothing.
    #[test]
    fn without_a_map_the_setting_changes_nothing() {
        let diagnostics = check_metadata_diagnostic_with_vendor_support(
            document(VENDOR_DOCUMENT, ModuleType::ObjectModule, None),
            CODE,
            config(Some(true)),
            None,
            check,
        );
        assert_eq!(format_diags(CODE, &diagnostics), REPORTED);
    }

    #[test]
    fn the_map_is_requested_only_by_the_setting() {
        let section = |value: serde_json::Value| serde_json::json!({ "parameters": value });
        assert!(vendor_support_requested(&section(
            serde_json::json!({"CommentedCode": {"skipSupported": true}})
        )));
        assert!(!vendor_support_requested(&section(
            serde_json::json!({"CommentedCode": {"skipSupported": false}})
        )));
        assert!(!vendor_support_requested(&section(serde_json::json!({"CommentedCode": false}))));
        assert!(!vendor_support_requested(&serde_json::Value::Null));
    }
}
