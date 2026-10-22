use parser_error::{ParseError, RecoveryKind};

use crate::{NodeOrToken, Parse, SyntaxKind, SyntaxNode, TextRange, TextSize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdblQueryInfo {
    pub bsl_literal_range: TextRange,

    pub query_text: String,

    pub query_ast: Option<Parse<SyntaxNode>>,

    pub quote_corrections: Vec<(usize, usize)>,

    pub error_ranges_in_bsl: Vec<(TextRange, ParseError)>,

    /// How the query text lies in the source: in a literal torn by extension
    /// markers, or in the literals of a constant concatenation it was joined from;
    /// `None` for an ordinary literal, whose query lies line for line.
    pub literal_map: Option<LiteralTextMap>,

    /// The literal holds a piece or a template of a query text rather than the text
    /// itself: it is concatenated with a value known only at run time, completed
    /// later by concatenation or substitution, or carries placeholders. Its parse
    /// errors are those of the piece, not of a query the platform executes.
    pub incomplete: bool,
}

/// The parsed query of one string literal without the literal's position —
/// what a lowered body stores, so that the body stays equal when the literal
/// moves. [`SdblQueryInfo`] is the same payload placed in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdblQuery {
    pub query_text: String,

    pub query_ast: Option<Parse<SyntaxNode>>,

    pub quote_corrections: Vec<(usize, usize)>,

    /// Parse errors in the coordinates of the literal text.
    pub error_ranges_in_bsl: Vec<(TextRange, ParseError)>,

    /// How the query text lies in the source: in a literal torn by extension
    /// markers, or in the literals of a constant concatenation it was joined from;
    /// `None` for an ordinary literal, whose query lies line for line.
    pub literal_map: Option<LiteralTextMap>,

    /// The literal holds a piece or a template of a query text rather than the text
    /// itself: it is concatenated with a value known only at run time, completed
    /// later by concatenation or substitution, or carries placeholders. Its parse
    /// errors are those of the piece, not of a query the platform executes.
    pub incomplete: bool,
}

impl SdblQuery {
    pub fn is_valid(&self) -> bool {
        self.query_ast.as_ref().map(|p| !p.has_errors()).unwrap_or(false)
    }

    pub fn syntax_node(&self) -> Option<SyntaxNode> {
        self.query_ast.as_ref().map(|p| p.syntax_node())
    }
}

impl SdblQueryInfo {
    pub fn from_query(bsl_literal_range: TextRange, query: &SdblQuery) -> Self {
        Self {
            bsl_literal_range,
            query_text: query.query_text.clone(),
            query_ast: query.query_ast.clone(),
            quote_corrections: query.quote_corrections.clone(),
            error_ranges_in_bsl: query.error_ranges_in_bsl.clone(),
            literal_map: query.literal_map.clone(),
            incomplete: query.incomplete,
        }
    }

    pub fn new(
        bsl_literal_range: TextRange,
        query_text: String,
        query_ast: Option<Parse<SyntaxNode>>,
        quote_corrections: Vec<(usize, usize)>,
        error_ranges_in_bsl: Vec<(TextRange, ParseError)>,
    ) -> Self {
        Self {
            bsl_literal_range,
            query_text,
            query_ast,
            quote_corrections,
            error_ranges_in_bsl,
            literal_map: None,
            incomplete: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.query_ast.as_ref().map(|p| !p.has_errors()).unwrap_or(false)
    }

    pub fn syntax_node(&self) -> Option<SyntaxNode> {
        self.query_ast.as_ref().map(|p| p.syntax_node())
    }
}

/// Every parse error of an SDBL query, in the coordinates of the query text itself.
///
/// The parser's own [`ParseError`]s, plus synthetic ones for shapes the tree accepts and the
/// platform's grammar does not: a reference path that ends on a dot, and a nested query in a
/// select list. `ERROR` nodes carry no `ParseError`, so a consumer that walks the tree instead
/// of calling this cannot render the same messages.
///
/// Callers embedding the query in a BSL literal map each range through
/// [`map_range_query_to_literal`]; a caller validating a bare query text uses them as they are.
pub fn collect_query_parse_errors(query_ast: &Parse<SyntaxNode>) -> Vec<(TextRange, ParseError)> {
    let mut errors: Vec<(TextRange, ParseError)> =
        query_ast.errors().iter().map(|err| (err.range(), err.structured().clone())).collect();

    let root = query_ast.syntax_node();
    for refs in root.descendants().filter(|node| node.kind() == SyntaxKind::SDBL_REFS_EXPR) {
        if let Some(dot_range) = trailing_dot_range(&refs) {
            errors.push((
                dot_range,
                ParseError::Custom {
                    message: "незавершённый путь в ссылке",
                    recovery: RecoveryKind::Custom,
                },
            ));
        }
    }

    for subquery in root.descendants().filter(|node| node.kind() == SyntaxKind::SDBL_SUBQUERY_EXPR)
    {
        if is_in_select_list(&subquery) {
            errors.push((
                subquery.text_range(),
                ParseError::Custom {
                    message: "вложенный запрос в списке полей выборки недопустим: он возможен \
                              только как источник в ИЗ и справа от В (...)",
                    recovery: RecoveryKind::Custom,
                },
            ));
        }
    }

    errors
}

/// Whether a nested query sits in a select list rather than in one of the two places the
/// platform accepts one inside an expression.
///
/// The parser reads `(ВЫБРАТЬ ...)` as an operand anywhere, as SQL does; 1C answers
/// «Синтаксическая ошибка "ВЫБРАТЬ"» at run time (8.3.17 and 8.3.27). Only the select-list
/// position is reported — the one attested. The walk up stops on reaching `В (...)` /
/// `В ИЕРАРХИИ (...)` from its right side, so the right side of `В` stays unreported wherever it
/// appears, while the left operand is an ordinary operand of the field. It also stops at the
/// first enclosing query, so a nested query in a condition of another nested query is judged by
/// the query it belongs to rather than by the select list that query happens to sit in.
fn is_in_select_list(subquery: &SyntaxNode) -> bool {
    let mut child = subquery.clone();
    for ancestor in subquery.ancestors().skip(1) {
        match ancestor.kind() {
            SyntaxKind::SDBL_IN_EXPR | SyntaxKind::SDBL_IN_HIERARCHY_EXPR
                if !is_left_of_in(&ancestor, &child) =>
            {
                return false;
            }
            SyntaxKind::SDBL_SELECTED_FIELD => return true,
            SyntaxKind::SDBL_SUBQUERY => return false,
            _ => {}
        }
        child = ancestor;
    }
    false
}

/// Whether `child` of an `В` / `В ИЕРАРХИИ` predicate lies before its `В` keyword, i.e. is the
/// tested operand rather than the set it is tested against.
fn is_left_of_in(predicate: &SyntaxNode, child: &SyntaxNode) -> bool {
    predicate
        .children_with_tokens()
        .find(|element| element.kind() == SyntaxKind::KW_IN)
        .is_some_and(|keyword| child.text_range().end() <= keyword.text_range().start())
}

fn trailing_dot_range(refs: &SyntaxNode) -> Option<TextRange> {
    for child in refs.children_with_tokens().collect::<Vec<_>>().into_iter().rev() {
        let kind = child.kind();
        if kind.is_trivia() {
            continue;
        }

        return if kind == SyntaxKind::DOT {
            child.as_token().map(|token| token.text_range())
        } else {
            None
        };
    }

    None
}

pub fn extract_sdbl_with_corrections(node: &SyntaxNode) -> Option<(String, Vec<(usize, usize)>)> {
    let mut result = String::new();
    let mut corrections = Vec::new();
    let mut tokens = node.children_with_tokens().filter_map(|it| it.into_token());

    let first_token = tokens.next()?;

    match first_token.kind() {
        SyntaxKind::STRING => {
            let text = first_token.text();
            if text.len() < 2 {
                return None;
            }
            let inner = &text[1..text.len() - 1];

            let mut pos = 0;
            let mut chars = inner.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '"' && chars.peek() == Some(&'"') {
                    chars.next();
                    result.push('"');
                    corrections.push((pos, 1));
                    pos += 1;
                } else {
                    result.push(ch);
                    pos += ch.len_utf8();
                }
            }
        }
        SyntaxKind::STRING_START => {
            let text = first_token.text();
            if text.is_empty() {
                return None;
            }

            let first_line = &text[1..];
            let mut pos = 0;
            let mut chars = first_line.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '"' && chars.peek() == Some(&'"') {
                    chars.next();
                    result.push('"');
                    corrections.push((pos, 1));
                    pos += 1;
                } else {
                    result.push(ch);
                    pos += ch.len_utf8();
                }
            }

            for token in tokens {
                match token.kind() {
                    SyntaxKind::NEWLINE => {
                        result.push('\n');
                        pos += 1;
                    }
                    SyntaxKind::STRING_PART | SyntaxKind::STRING_TAIL => {
                        let text = token.text();
                        let content = if let Some(c) = text.strip_prefix('|') {
                            if token.kind() == SyntaxKind::STRING_TAIL {
                                c.strip_suffix('"').unwrap_or(c)
                            } else {
                                c
                            }
                        } else {
                            text
                        };

                        let mut chars = content.chars().peekable();
                        while let Some(ch) = chars.next() {
                            if ch == '"' && chars.peek() == Some(&'"') {
                                chars.next();
                                result.push('"');
                                corrections.push((pos, 1));
                                pos += 1;
                            } else {
                                result.push(ch);
                                pos += ch.len_utf8();
                            }
                        }

                        if token.kind() == SyntaxKind::STRING_TAIL {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => return None,
    }

    Some((result, corrections))
}

/// Where one piece of the active text lies in the literal: `text_len` bytes of
/// the text at `text_start` come from `literal_len` bytes of the literal at
/// `literal_start`. A doubled quote is two literal bytes for one, a line break
/// is the newline, the indent and the `|` of the next line for one `\n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MapUnit {
    text_start: u32,
    text_len: u32,
    literal_start: u32,
    literal_len: u32,
}

/// The text a string literal holds once the extension markers inside it are
/// applied, and where each of its bytes stands in the literal.
///
/// The platform applies `#Вставка` and `#Удаление` to the module text, so a
/// literal they tear apart holds its inserted lines and none of the removed
/// ones — and the markers are no part of it. Line arithmetic cannot place such
/// a text back in the source: a query line is no longer the literal line of the
/// same number, and the lines in between are not text of the literal. The map
/// is built together with the text so that every consumer places a position
/// the same way.
///
/// Offsets on the literal side are relative to the literal's start, so the map
/// stays equal when the literal moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralTextMap {
    units: Vec<MapUnit>,
    text_len: u32,
    /// The end of the active content: where the closing quote stands, or where
    /// the last active line ends when the literal is not closed.
    content_end: u32,
}

impl LiteralTextMap {
    /// Places a range of the active text in the literal.
    ///
    /// A non-empty range covers the source bytes of every character it holds;
    /// an empty one stands before the character at its position, and past the
    /// last character — before the closing quote.
    pub fn map_range_to_literal(&self, range: TextRange) -> TextRange {
        let start = self.literal_position(u32::from(range.start()));
        if range.is_empty() {
            return TextRange::empty(start.into());
        }
        let end = match self.unit_at_text(u32::from(range.end()) - 1) {
            Some(unit) => unit.literal_start + unit.literal_len,
            None => self.content_end,
        };
        TextRange::new(start.into(), end.max(start).into())
    }

    /// The offset in the active text that a literal offset stands at, or
    /// `None` where the literal holds no active text — on a marker, or in what
    /// an extension removed.
    pub fn map_offset_to_text(&self, literal_offset: TextSize) -> Option<TextSize> {
        let offset = u32::from(literal_offset);
        if offset == self.content_end {
            return Some(self.text_len.into());
        }
        let index =
            self.units.partition_point(|unit| unit.literal_start <= offset).checked_sub(1)?;
        let unit = self.units[index];
        if offset >= unit.literal_start + unit.literal_len {
            return None;
        }
        let inside =
            if unit.text_len == unit.literal_len { offset - unit.literal_start } else { 0 };
        Some((unit.text_start + inside).into())
    }

    fn literal_position(&self, text_offset: u32) -> u32 {
        self.unit_at_text(text_offset).map_or(self.content_end, |unit| unit.literal_start)
    }

    fn unit_at_text(&self, text_offset: u32) -> Option<MapUnit> {
        if text_offset >= self.text_len {
            return None;
        }
        let index = self.units.partition_point(|unit| unit.text_start <= text_offset);
        index.checked_sub(1).map(|index| self.units[index])
    }
}

/// Whether the literal is torn apart by extension markers — whether its text
/// has to be read through [`extract_torn_literal`].
pub fn is_torn_literal(literal: &SyntaxNode) -> bool {
    literal.kind() == SyntaxKind::LITERAL
        && literal
            .children_with_tokens()
            .any(|child| crate::ast_utils::is_extension_directive(&child))
}

/// The active text of a literal torn by extension markers, with its map; `None`
/// for any other literal.
///
/// Inserted lines are part of the text, removed ones are not, and a closing
/// line inside a removed block does not close the literal. What looks like a
/// marker inside a line of the literal is its text: the lexer takes a line of a
/// literal whole.
pub fn extract_torn_literal(literal: &SyntaxNode) -> Option<(String, LiteralTextMap)> {
    if !is_torn_literal(literal) {
        return None;
    }
    let mut builder = TornLiteralBuilder::default();
    push_literal_text(&mut builder, literal, literal.text_range().start()).then(|| builder.finish())
}

/// One operand of a constant string concatenation: a string literal, or a
/// character constant such as `Символы.ПС` standing at `range`.
#[derive(Debug, Clone)]
pub enum ConcatPiece {
    Literal(SyntaxNode),
    Char(char, TextRange),
}

/// The text a constant concatenation of strings evaluates to, and where each of
/// its bytes stands in the source, relative to the start of the first literal.
///
/// `"ВЫБРАТЬ 1 КАК Один," + " 2 КАК Два"` is one query to the platform, not two
/// literals: only the joined text says whether it parses, and only the map
/// places an error of the second literal in the second literal. `None` when the
/// concatenation has no literal or a literal holds no text. Leading character
/// constants map to the first literal's opening quote, since relative source
/// offsets cannot represent positions before that literal.
pub fn extract_concatenated_text(pieces: &[ConcatPiece]) -> Option<(String, LiteralTextMap)> {
    let first = pieces.iter().find_map(|piece| match piece {
        ConcatPiece::Literal(literal) => Some(literal),
        ConcatPiece::Char(..) => None,
    })?;
    let base = first.text_range().start();
    let mut builder = TornLiteralBuilder::default();
    for piece in pieces {
        match piece {
            ConcatPiece::Literal(literal) => {
                if !push_literal_text(&mut builder, literal, base) {
                    return None;
                }
            }
            ConcatPiece::Char(ch, range) => {
                let start = u32::from(range.start()).saturating_sub(u32::from(base));
                let len = if range.start() < base { 0 } else { u32::from(range.len()) };
                builder.push_unit(*ch, start, len);
            }
        }
    }
    Some(builder.finish())
}

/// Appends the active text of one string literal to `builder`, its positions
/// relative to `base`. `false` when the node holds no string.
fn push_literal_text(
    builder: &mut TornLiteralBuilder,
    literal: &SyntaxNode,
    base: TextSize,
) -> bool {
    let relative = |offset: TextSize| u32::from(offset - base);
    let mut started = false;
    let mut line_break_start = None;

    for child in literal.children_with_tokens() {
        let token = match child {
            NodeOrToken::Node(_) => {
                line_break_start = None;
                continue;
            }
            NodeOrToken::Token(token) => token,
        };
        let start = relative(token.text_range().start());
        match token.kind() {
            SyntaxKind::STRING if !started => {
                let text = token.text();
                let content = text[1..].strip_suffix('"').unwrap_or(&text[1..]);
                builder.push_content(content, start + 1);
                return true;
            }
            SyntaxKind::STRING_START if !started => {
                started = true;
                builder.push_content(&token.text()[1..], start + 1);
            }
            SyntaxKind::STRING_PART | SyntaxKind::STRING_TAIL if started => {
                let text = token.text();
                let break_start = line_break_start.take().unwrap_or(start);
                builder.push_unit('\n', break_start, start + 1 - break_start);
                let content = &text[1..];
                let content = if token.kind() == SyntaxKind::STRING_TAIL {
                    content.strip_suffix('"').unwrap_or(content)
                } else {
                    content
                };
                builder.push_content(content, start + 1);
                if token.kind() == SyntaxKind::STRING_TAIL {
                    break;
                }
            }
            SyntaxKind::NEWLINE => line_break_start = Some(start),
            SyntaxKind::PRE_INSERT | SyntaxKind::PRE_END_INSERT => line_break_start = None,
            kind if kind.is_trivia() => {}
            _ => break,
        }
    }

    started
}

#[derive(Default)]
struct TornLiteralBuilder {
    text: String,
    units: Vec<MapUnit>,
    content_end: u32,
}

impl TornLiteralBuilder {
    fn push_content(&mut self, content: &str, literal_start: u32) {
        let mut chars = content.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            let at = literal_start + index as u32;
            if ch == '"' && chars.peek().is_some_and(|&(_, next)| next == '"') {
                chars.next();
                self.push_unit('"', at, 2);
            } else {
                self.push_unit(ch, at, ch.len_utf8() as u32);
            }
        }
        self.content_end = literal_start + content.len() as u32;
    }

    fn push_unit(&mut self, ch: char, literal_start: u32, literal_len: u32) {
        let text_start = self.text.len() as u32;
        self.text.push(ch);
        self.units.push(MapUnit {
            text_start,
            text_len: ch.len_utf8() as u32,
            literal_start,
            literal_len,
        });
        self.content_end = literal_start + literal_len;
    }

    fn finish(self) -> (String, LiteralTextMap) {
        let text_len = self.text.len() as u32;
        (self.text, LiteralTextMap { units: self.units, text_len, content_end: self.content_end })
    }
}

pub fn map_range_query_to_literal(literal_text: &str, query_range: TextRange) -> TextRange {
    let query_start: u32 = query_range.start().into();
    let query_end: u32 = query_range.end().into();
    let bytes = literal_text.as_bytes();

    let mut sdbl_byte = 0u32;
    let mut projection = RangeProjectionState::new(query_start, query_end);

    let Some(first_line) = next_line(bytes, 0) else {
        projection.record_boundary(0, 0, 0);
        let start = projection.projected_start.unwrap_or(0);
        let end = if query_start == query_end {
            start
        } else {
            projection.projected_end.unwrap_or(start)
        };
        return TextRange::new(start.into(), end.into());
    };

    let mut content_start = skip_leading_whitespace(literal_text, first_line.start, first_line.end);
    if content_start < first_line.end && bytes[content_start] == b'"' {
        content_start += 1;
    }
    projection.record_boundary(0, content_start as u32, content_start as u32);

    let mut content_end = line_content_end(bytes, first_line);
    process_content(literal_text, content_start, content_end, &mut sdbl_byte, &mut projection);

    let mut pending_linebreak_start =
        (first_line.next_start > first_line.end).then_some(first_line.end);
    let mut line_start = first_line.next_start;

    while line_start < bytes.len() {
        let Some(line) = next_line(bytes, line_start) else {
            break;
        };

        let trimmed_start = skip_leading_whitespace(literal_text, line.start, line.end);

        if bytes[trimmed_start..line.end].starts_with(b"//") {
            line_start = line.next_start;
            continue;
        }

        if let Some(linebreak_start) = pending_linebreak_start.take() {
            let (line_content_start, linebreak_end) =
                if trimmed_start < line.end && bytes[trimmed_start] == b'|' {
                    (trimmed_start + 1, trimmed_start + 1)
                } else {
                    (line.start, line.start)
                };

            projection.emit_segment(
                &mut sdbl_byte,
                1,
                linebreak_start,
                linebreak_end,
                linebreak_start,
            );

            content_end = line_content_end(bytes, line);
            process_content(
                literal_text,
                line_content_start,
                content_end,
                &mut sdbl_byte,
                &mut projection,
            );
        }

        pending_linebreak_start = (line.next_start > line.end).then_some(line.end);
        line_start = line.next_start;
    }

    let fallback = content_end as u32;
    projection.record_boundary(sdbl_byte, fallback, fallback);

    let start = projection.projected_start.unwrap_or(fallback);
    let mut end =
        if query_start == query_end { start } else { projection.projected_end.unwrap_or(fallback) };

    if end < start {
        end = start;
    }
    debug_assert!(start <= end, "sdbl_query projection produced inverted range");

    TextRange::new(start.into(), end.into())
}

struct RangeProjectionState {
    query_start: u32,
    query_end: u32,
    projected_start: Option<u32>,
    projected_end: Option<u32>,
}

impl RangeProjectionState {
    fn new(query_start: u32, query_end: u32) -> Self {
        Self { query_start, query_end, projected_start: None, projected_end: None }
    }

    fn record_boundary(&mut self, sdbl_byte: u32, bsl_start_bias: u32, bsl_end_bias: u32) {
        if self.projected_start.is_none() && sdbl_byte == self.query_start {
            self.projected_start = Some(bsl_start_bias);
        }
        if self.projected_end.is_none() && sdbl_byte == self.query_end {
            self.projected_end = Some(bsl_end_bias);
        }
    }

    fn emit_segment(
        &mut self,
        sdbl_byte: &mut u32,
        sdbl_len: u32,
        bsl_start: usize,
        bsl_end: usize,
        next_start_bias: usize,
    ) {
        let bsl_start = bsl_start as u32;
        let bsl_end = bsl_end as u32;
        let next_start_bias = next_start_bias as u32;

        self.record_boundary(*sdbl_byte, bsl_start, bsl_start);
        *sdbl_byte += sdbl_len;
        self.record_boundary(*sdbl_byte, next_start_bias, bsl_end);
    }
}

#[derive(Clone, Copy)]
struct LiteralLine {
    start: usize,
    end: usize,
    next_start: usize,
}

fn next_line(bytes: &[u8], start: usize) -> Option<LiteralLine> {
    if start > bytes.len() {
        return None;
    }

    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }

    if end < bytes.len() {
        let line_end = if end > start && bytes[end - 1] == b'\r' { end - 1 } else { end };
        Some(LiteralLine { start, end: line_end, next_start: end + 1 })
    } else if start < bytes.len() {
        Some(LiteralLine { start, end, next_start: end })
    } else {
        None
    }
}

fn line_content_end(bytes: &[u8], line: LiteralLine) -> usize {
    if line.next_start == line.end && line.end > line.start && bytes[line.end - 1] == b'"' {
        line.end - 1
    } else {
        line.end
    }
}

fn skip_leading_whitespace(text: &str, start: usize, end: usize) -> usize {
    debug_assert!(text.is_char_boundary(start), "start must be on a UTF-8 char boundary");
    debug_assert!(text.is_char_boundary(end), "end must be on a UTF-8 char boundary");
    let mut pos = start;
    while pos < end {
        let ch = text[pos..end].chars().next().expect("valid UTF-8 string slice");
        if !ch.is_whitespace() {
            break;
        }
        pos += ch.len_utf8();
    }
    pos
}

fn process_content(
    literal_text: &str,
    start: usize,
    end: usize,
    sdbl_byte: &mut u32,
    projection: &mut RangeProjectionState,
) {
    debug_assert!(
        literal_text.is_char_boundary(start),
        "process_content start must be on a UTF-8 char boundary"
    );
    debug_assert!(
        literal_text.is_char_boundary(end),
        "process_content end must be on a UTF-8 char boundary"
    );
    let bytes = literal_text.as_bytes();
    let mut pos = start;

    while pos < end {
        if pos + 1 < end && bytes[pos] == b'"' && bytes[pos + 1] == b'"' {
            projection.emit_segment(sdbl_byte, 1, pos, pos + 2, pos + 2);
            pos += 2;
        } else {
            let ch = literal_text[pos..end].chars().next().expect("valid UTF-8 string slice");
            let len = ch.len_utf8();
            projection.emit_segment(sdbl_byte, len as u32, pos, pos + len, pos + len);
            pos += len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdbl_query_info_creation() {
        let range = TextRange::new(0u32.into(), 10u32.into());
        let query_text = "SELECT * FROM Table".to_string();

        let info = SdblQueryInfo::new(range, query_text.clone(), None, vec![], vec![]);

        assert_eq!(info.bsl_literal_range, range);
        assert_eq!(info.query_text, query_text);
        assert!(info.error_ranges_in_bsl.is_empty());
        assert!(!info.is_valid());
        assert!(info.syntax_node().is_none());
    }

    #[test]
    fn test_is_valid_with_no_ast() {
        let info = SdblQueryInfo::new(
            TextRange::new(0u32.into(), 10u32.into()),
            "INVALID QUERY".to_string(),
            None,
            vec![],
            vec![],
        );
        assert!(!info.is_valid());
        assert!(info.syntax_node().is_none());
    }

    #[test]
    fn test_clone_and_equality() {
        let query = "SELECT Name AS N FROM Table";

        let info1 = SdblQueryInfo::new(
            TextRange::new(0u32.into(), 10u32.into()),
            query.to_string(),
            None,
            vec![],
            vec![],
        );

        let info2 = info1.clone();

        assert_eq!(info1, info2);
        assert_eq!(info1.bsl_literal_range, info2.bsl_literal_range);
        assert_eq!(info1.query_text, info2.query_text);
    }

    #[test]
    fn test_error_ranges_in_bsl_empty_round_trip() {
        let info = SdblQueryInfo::new(
            TextRange::new(3u32.into(), 42u32.into()),
            "SELECT * FROM Table".to_string(),
            None,
            vec![],
            vec![],
        );

        assert!(info.error_ranges_in_bsl.is_empty());
    }

    #[test]
    fn test_error_ranges_in_bsl_preserves_structured_parse_error() {
        let range = TextRange::new(5u32.into(), 6u32.into());
        let error = ParseError::Custom {
            message: "незавершённый путь в ссылке",
            recovery: parser_error::RecoveryKind::Custom,
        };

        let info = SdblQueryInfo::new(
            TextRange::new(0u32.into(), 20u32.into()),
            "SELECT * FROM Table".to_string(),
            None,
            vec![],
            vec![(range, error.clone())],
        );

        assert_eq!(info.error_ranges_in_bsl, vec![(range, error)]);
        assert!(matches!(
            &info.error_ranges_in_bsl[0].1,
            ParseError::Custom {
                message: "незавершённый путь в ссылке",
                recovery: parser_error::RecoveryKind::Custom,
            }
        ));
    }
}

#[cfg(test)]
mod tests_range_projection {
    use super::*;

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(start.into(), end.into())
    }

    #[test]
    fn simple_literal_projects_after_opening_quote() {
        let literal = "\"X = 1\"";

        assert_eq!(map_range_query_to_literal(literal, range(0, 1)), range(1, 2));
    }

    #[test]
    fn empty_query_range_projects_to_zero_width_literal_range() {
        let literal = "\"X = 1\"";

        assert_eq!(map_range_query_to_literal(literal, range(2, 2)), range(3, 3));
    }

    #[test]
    fn multiline_pipe_projection_includes_newline_and_pipe_prefix() {
        let literal = "\"VYIBRAT X\n|IZ T\"";

        assert_eq!(map_range_query_to_literal(literal, range(10, 12)), range(10, 14));
    }

    #[test]
    fn escaped_quotes_projection_spans_doubled_quote_bytes() {
        let literal = "\"X = \"\"Y\"\"\"";

        assert_eq!(map_range_query_to_literal(literal, range(4, 7)), range(5, 10));
    }

    #[test]
    fn cyrillic_and_escaped_quotes_project_on_utf8_byte_boundaries() {
        let literal = "\"Поле = \"\"Я\"\"\"";

        assert_eq!(map_range_query_to_literal(literal, range(11, 15)), range(12, 18));
    }

    #[test]
    fn crlf_pipe_projection_includes_crlf_and_pipe_prefix() {
        let literal = "\"X\r\n|Y\"";

        assert_eq!(map_range_query_to_literal(literal, range(2, 3)), range(2, 6));
    }

    #[test]
    fn bsl_comment_continuation_projection_includes_comment_bytes() {
        let literal = "\"X\n// comment\n|Y\"";

        assert_eq!(
            map_range_query_to_literal(literal, range(2, 3)),
            range(2, (literal.len() - 1) as u32),
        );
    }

    #[test]
    fn out_of_bounds_query_end_does_not_invert_projection() {
        let literal = "\"X\n|Y\n|Z\"";
        let projected = map_range_query_to_literal(literal, range(3, 1000));
        assert!(
            projected.start() <= projected.end(),
            "projection produced inverted range: {:?}",
            projected,
        );
    }
}
