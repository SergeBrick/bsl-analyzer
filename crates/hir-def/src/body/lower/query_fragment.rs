//! Whether a string literal holds a whole query text or only a piece of one.
//!
//! A query is often assembled rather than written: `"ВЫБРАТЬ ... ИЗ " + Имя + " КАК Т"`,
//! a text completed by `Текст = Текст + ...` further down, a template whose `#Таблица` or `%1`
//! is substituted before execution. The platform executes the assembled text, never the
//! literal alone, so the literal's own parse errors say nothing about the query.

use stdx::case::CaseExt;
use syntax::ast_utils::active_children;
use syntax::sdbl_query::ConcatPiece;
use syntax::{SyntaxKind, SyntaxNode};

/// The place of a string literal in a concatenation.
pub(super) enum ConcatRole {
    /// Not an operand of `+`.
    Alone,
    /// The first operand of a concatenation all of whose operands are constant strings; the
    /// pieces are the operands in order.
    Head(Vec<ConcatPiece>),
    /// A later operand of such a concatenation: its text is part of the head's.
    Tail,
    /// An operand of a concatenation with a value known only at run time.
    Dynamic,
}

pub(super) fn concat_role(literal: &SyntaxNode) -> ConcatRole {
    let mut current = literal.clone();
    let mut root = None;
    let mut in_branch = false;
    loop {
        let top = unwrap_up(&current);
        let Some(parent) = top.parent() else { break };
        if is_concatenation(&parent) {
            // `"..." + ?(Флаг, "А", "Б")`: a branch is chosen at run time.
            if in_branch {
                return ConcatRole::Dynamic;
            }
            root = Some(parent.clone());
            current = parent;
        } else if parent.kind() == SyntaxKind::TERNARY_EXPR && !is_ternary_condition(&parent, &top)
        {
            in_branch = true;
            current = parent;
        } else {
            break;
        }
    }
    let Some(root) = root else { return ConcatRole::Alone };
    let mut operands = Vec::new();
    collect_operands(&root, &mut operands);
    let mut pieces = Vec::with_capacity(operands.len());
    for operand in &operands {
        match constant_piece(operand) {
            Some(piece) => pieces.push(piece),
            None => return ConcatRole::Dynamic,
        }
    }
    match pieces.first() {
        Some(ConcatPiece::Literal(first)) if first == literal => ConcatRole::Head(pieces),
        Some(ConcatPiece::Literal(_)) => ConcatRole::Tail,
        _ => ConcatRole::Dynamic,
    }
}

/// Whether the literal is not the text a query executes: it carries template placeholders or
/// the quotes of another SQL dialect, is an argument of a substitution, or is stored in a
/// variable (or `Запрос.Текст`) that the same body later concatenates or passes to a
/// substitution.
pub(super) fn is_completed_elsewhere(literal: &SyntaxNode, query_text: &str) -> bool {
    has_foreign_markers(query_text) || is_substitution_source(literal)
}

fn is_substitution_source(literal: &SyntaxNode) -> bool {
    let mut top = unwrap_up(literal);
    while let Some(parent) = top.parent() {
        if parent.kind() == SyntaxKind::TERNARY_EXPR && !is_ternary_condition(&parent, &top) {
            top = unwrap_up(&parent);
        } else {
            break;
        }
    }
    let Some(parent) = top.parent() else { return false };
    let holder = match parent.kind() {
        SyntaxKind::ARG_LIST => {
            let Some(call) = parent.parent() else { return false };
            if is_substitution_call(&call) {
                return true;
            }
            // `Запрос = Новый Запрос("...")`: the text lives on in `Запрос.Текст`.
            if call.kind() != SyntaxKind::NEW_EXPR || !is_query_constructor(&call) {
                return false;
            }
            match assignment_target(&unwrap_up(&call)) {
                Some(target) => format!("{}.текст", normalized(&target)),
                None => return false,
            }
        }
        SyntaxKind::ASSIGN_STMT => match assignment_target(&top) {
            Some(target) => normalized(&target),
            None => return false,
        },
        _ => return false,
    };
    let holders = equivalent_holders(&holder);
    let Some(body) = literal.ancestors().last() else { return false };
    body.descendants().any(|node| {
        if is_concatenation(&node) {
            let mut operands = Vec::new();
            collect_operands(&node, &mut operands);
            operands.iter().any(|operand| holders.contains(&normalized(operand)))
        } else if node.kind() == SyntaxKind::CALL_EXPR && is_substitution_call(&node) {
            first_argument(&node).is_some_and(|arg| holders.contains(&normalized(&arg)))
        } else {
            false
        }
    })
}

/// `Запрос.Текст` and `Запрос.Text` name the same property.
fn equivalent_holders(holder: &str) -> Vec<String> {
    let mut holders = vec![holder.to_string()];
    for (from, to) in [(".текст", ".text"), (".text", ".текст")] {
        if let Some(stem) = holder.strip_suffix(from) {
            holders.push(format!("{stem}{to}"));
        }
    }
    holders
}

/// The target of the assignment whose right-hand side is `value`.
fn assignment_target(value: &SyntaxNode) -> Option<SyntaxNode> {
    let stmt = value.parent().filter(|p| p.kind() == SyntaxKind::ASSIGN_STMT)?;
    let mut children = active_children(&stmt);
    let target = children.next()?;
    (children.next().as_ref() == Some(value)).then_some(target)
}

fn is_concatenation(node: &SyntaxNode) -> bool {
    node.kind() == SyntaxKind::BINARY_EXPR
        && node
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|tok| tok.kind() == SyntaxKind::PLUS)
}

/// The outermost wrapper (`EXPR`, parentheses) whose only content is `node`.
fn unwrap_up(node: &SyntaxNode) -> SyntaxNode {
    let mut current = node.clone();
    while let Some(parent) = current.parent() {
        if matches!(parent.kind(), SyntaxKind::EXPR | SyntaxKind::PAREN_EXPR) {
            current = parent;
        } else {
            break;
        }
    }
    current
}

/// The node an `EXPR` or parentheses wrapper stands for.
fn unwrap_down(node: &SyntaxNode) -> SyntaxNode {
    let mut current = node.clone();
    while matches!(current.kind(), SyntaxKind::EXPR | SyntaxKind::PAREN_EXPR) {
        match active_children(&current).next() {
            Some(child) => current = child,
            None => break,
        }
    }
    current
}

/// The operands of a chain of `+`, left to right, each unwrapped.
fn collect_operands(concatenation: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
    for child in active_children(concatenation) {
        let operand = unwrap_down(&child);
        if is_concatenation(&operand) {
            collect_operands(&operand, out);
        } else {
            out.push(operand);
        }
    }
}

fn constant_piece(operand: &SyntaxNode) -> Option<ConcatPiece> {
    match operand.kind() {
        SyntaxKind::LITERAL => operand
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|tok| matches!(tok.kind(), SyntaxKind::STRING | SyntaxKind::STRING_START))
            .then(|| ConcatPiece::Literal(operand.clone())),
        SyntaxKind::FIELD_EXPR => {
            let ch = match normalized(operand).as_str() {
                "символы.пс" | "chars.lf" => '\n',
                "символы.вк" | "chars.cr" => '\r',
                "символы.таб" | "chars.tab" => '\t',
                _ => return None,
            };
            Some(ConcatPiece::Char(ch, operand.text_range()))
        }
        _ => None,
    }
}

fn is_ternary_condition(ternary: &SyntaxNode, child: &SyntaxNode) -> bool {
    active_children(ternary).next().as_ref() == Some(child)
}

fn first_argument(call: &SyntaxNode) -> Option<SyntaxNode> {
    let args = active_children(call).find(|n| n.kind() == SyntaxKind::ARG_LIST)?;
    active_children(&args).next().map(|arg| unwrap_down(&arg))
}

/// `СтрЗаменить`, `СтрШаблон` and the library's `ПодставитьПараметрыВСтроку`, called by
/// name or through a module.
fn is_substitution_call(call: &SyntaxNode) -> bool {
    if call.kind() != SyntaxKind::CALL_EXPR {
        return false;
    }
    let Some(callee) = active_children(call).next() else { return false };
    let callee = normalized(&unwrap_down(&callee));
    let name = callee.rsplit('.').next().unwrap_or(&callee);
    matches!(
        name,
        "стрзаменить"
            | "strreplace"
            | "стршаблон"
            | "strtemplate"
            | "подставитьпараметрывстроку"
            | "insertparametersintostring"
    )
}

fn is_query_constructor(new_expr: &SyntaxNode) -> bool {
    new_expr
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|tok| tok.kind() == SyntaxKind::IDENT)
        .is_some_and(|tok| matches!(tok.text().fold_lower().as_str(), "запрос" | "query"))
}

/// The node's text without whitespace and comments, case-folded: `Запрос .Текст` and
/// `запрос.текст` are the same operand.
fn normalized(node: &SyntaxNode) -> String {
    let mut text = String::new();
    for token in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if !token.kind().is_trivia() {
            text.push_str(token.text());
        }
    }
    text.fold_lower()
}

/// Placeholders a template carries for text substituted before execution — `#Имя`, `%1`,
/// `%Имя%`, `[Имя]` — and the single quote WQL and SQL texts delimit strings with. None of them
/// is part of the query language: outside a string and a comment of the query the platform
/// rejects each, `'20200101'` included (8.3.17 and 8.3.27).
fn has_foreign_markers(query_text: &str) -> bool {
    let mut chars = query_text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                // A string of the query: `""` inside it is one quote.
                while let Some(c) = chars.next() {
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '#' | '%' | '[' => {
                if chars.peek().is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                    return true;
                }
            }
            '\'' => return true,
            _ => {}
        }
    }
    false
}
