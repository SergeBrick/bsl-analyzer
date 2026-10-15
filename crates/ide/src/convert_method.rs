use hir::Semantics;
use ide_assists::{Assist, AssistId, FileEdit, SourceChange, TextEdit};
use ide_db::RootDatabase;
use syntax::ast::{AstNode, FunctionDef, ProcedureDef};
use syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};
use vfs::FileId;

use crate::references::find_references_in_file;

/// `Процедура` ⇄ `Функция` on the declaration line of a method.
///
/// Only the opening and closing keywords are rewritten, in the script the source already
/// uses (`Процедура`/`Procedure`); annotations, `Асинх`, the parameter list, `Экспорт` and
/// the body are left as they are. A procedure becomes a function without a `Возврат` being
/// added: a function that never returns a value is valid and returns `Неопределено`.
///
/// The reverse is offered only when nothing depends on the function returning a value:
/// it has no `Возврат <выражение>` (a procedure does not compile one), no call in any body
/// of the module — the configuration's or an extension's — uses its result (a procedure
/// call is not an expression), those bodies run no dynamic code (`Вычислить`,
/// `Выполнить`), and it is not `Экспорт` — callers in other modules are not searched here,
/// so an exported function is never converted.
///
/// Neither direction is offered for a method an extension binds by name, nor for an
/// extension's interceptor itself: see [`bound_by_extension`].
pub fn convert_method_assists<DB: RootDatabase>(
    db: &DB,
    file_id: FileId,
    range: TextRange,
) -> Vec<Assist> {
    let text = db.file_text(file_id);
    let root = db.parse(file_id).syntax_node();

    let Some(method) = enclosing_method(&root, range.start()) else {
        return Vec::new();
    };
    if !on_declaration_line(&text, &method, range.start())
        || bound_by_extension(db, file_id, &method)
    {
        return Vec::new();
    }

    let assist = match method.kind() {
        SyntaxKind::PROCEDURE_DEF => convert(
            file_id,
            &method,
            (SyntaxKind::KW_PROCEDURE, SyntaxKind::KW_END_PROCEDURE),
            (("Функция", "КонецФункции"), ("Function", "EndFunction")),
            AssistId("convert_procedure_to_function"),
            "Преобразовать в функцию",
        ),
        SyntaxKind::FUNCTION_DEF if may_become_procedure(db, file_id, &method) => convert(
            file_id,
            &method,
            (SyntaxKind::KW_FUNCTION, SyntaxKind::KW_END_FUNCTION),
            (("Процедура", "КонецПроцедуры"), ("Procedure", "EndProcedure")),
            AssistId("convert_function_to_procedure"),
            "Преобразовать в процедуру",
        ),
        _ => None,
    };
    assist.into_iter().collect()
}

fn enclosing_method(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    let token = root.token_at_offset(offset).right_biased()?;
    token
        .parent_ancestors()
        .find(|n| matches!(n.kind(), SyntaxKind::PROCEDURE_DEF | SyntaxKind::FUNCTION_DEF))
}

fn keyword(method: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    method.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == kind)
}

/// The line that starts with `Асинх`/`Процедура`/`Функция`, up to its end. The method node
/// also spans the annotations above it (`&НаСервере`), which are not the declaration.
fn on_declaration_line(text: &str, method: &SyntaxNode, offset: TextSize) -> bool {
    let Some(start) = keyword(method, SyntaxKind::KW_ASYNC)
        .or_else(|| keyword(method, SyntaxKind::KW_PROCEDURE))
        .or_else(|| keyword(method, SyntaxKind::KW_FUNCTION))
        .map(|kw| usize::from(kw.text_range().start()))
    else {
        return false;
    };
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    (start..=end).contains(&usize::from(offset))
}

/// An extension binds a method by its name and its kind: `&Вместо` must be of the kind of
/// the method it replaces, `&Перед` and `&После` cannot extend a function, and
/// `&ИзменениеИКонтроль` must match the method's text. Converting either side of such a pair
/// leaves the interception silently inapplicable, and the other side lives in another body
/// of the same module, which the edit does not touch.
fn bound_by_extension<DB: RootDatabase>(db: &DB, file_id: FileId, method: &SyntaxNode) -> bool {
    if extension_target(method).is_some() {
        return true;
    }
    let Some(name) = method_name(method) else {
        return false;
    };
    let name = name.text().to_lowercase();
    sibling_roots(db, file_id).iter().any(|root| {
        // A method under `#Если` is a descendant of the directive, not a child of the file.
        root.descendants()
            .filter(|n| matches!(n.kind(), SyntaxKind::PROCEDURE_DEF | SyntaxKind::FUNCTION_DEF))
            .filter_map(|other| extension_target(&other))
            .any(|target| target.to_lowercase() == name)
    })
}

/// The other bodies of this module: the configuration's and those of its extensions.
fn sibling_roots<DB: RootDatabase>(db: &DB, file_id: FileId) -> Vec<SyntaxNode> {
    let source_root = db.file_source_root_input(file_id).source_root_id(db);
    db.module_index(source_root)
        .sibling_bodies(file_id)
        .into_iter()
        .map(|sibling| db.parse(sibling).syntax_node())
        .collect()
}

/// The base method an extension method intercepts or changes, by name.
fn extension_target(method: &SyntaxNode) -> Option<String> {
    hir::interceptor_target(method)
        .map(|interception| interception.target)
        .or_else(|| hir::extract_change_and_validate(method).map(|change| change.target))
}

fn method_name(method: &SyntaxNode) -> Option<SyntaxToken> {
    match ProcedureDef::cast(method.clone()) {
        Some(procedure) => procedure.name(),
        None => FunctionDef::cast(method.clone())?.name(),
    }
}

fn may_become_procedure<DB: RootDatabase>(db: &DB, file_id: FileId, method: &SyntaxNode) -> bool {
    let Some(function) = FunctionDef::cast(method.clone()) else {
        return false;
    };
    if function.export_keyword().is_some() || returns_value(method) {
        return false;
    }
    let Some(name) = function.name() else {
        return false;
    };
    let Some(symbol) = Semantics::new(db).symbol_for_token(file_id, &name) else {
        return false;
    };
    let root = db.parse(file_id).syntax_node();
    if runs_dynamic_code(&root) {
        return false;
    }
    let read_here = find_references_in_file(db, file_id, &symbol)
        .into_iter()
        .filter(|reference| reference.range != name.text_range())
        .any(|reference| {
            !root
                .token_at_offset(reference.range.start())
                .right_biased()
                .is_some_and(|token| is_call_statement(&token))
        });
    // An extension body resolves the base module's methods, `Экспорт` or not, so its calls
    // are callers too. They are matched by name: a same-named method of that body shadowing
    // this one only keeps the function as it is.
    !read_here
        && sibling_roots(db, file_id)
            .iter()
            .all(|sibling| !runs_dynamic_code(sibling) && !reads_by_name(sibling, name.text()))
}

fn reads_by_name(root: &SyntaxNode, name: &str) -> bool {
    let name = name.to_lowercase();
    root.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind().is_name_token() && token.text().to_lowercase() == name)
        .any(|token| !is_call_statement(&token))
}

/// `Выполнить` and `Вычислить` run text that is known only at run time, however it is
/// assembled, and it may call the function and read its result. A function without
/// `Экспорт` is reachable only from the bodies of its own module.
fn runs_dynamic_code(root: &SyntaxNode) -> bool {
    root.descendants().any(|node| match node.kind() {
        SyntaxKind::EXECUTE_STMT => true,
        SyntaxKind::CALL_EXPR => node.first_child().is_some_and(|callee| {
            let callee = match callee.kind() {
                SyntaxKind::EXPR => callee.first_child().unwrap_or(callee),
                _ => callee,
            };
            callee.kind() == SyntaxKind::IDENT && {
                let name = callee.text().to_string().to_lowercase();
                name == "вычислить" || name == "eval"
            }
        }),
        _ => false,
    })
}

/// A bare `Возврат;` holds only tokens; `Возврат <выражение>;` holds the expression node.
fn returns_value(method: &SyntaxNode) -> bool {
    method
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::RETURN_STMT)
        .any(|ret| ret.children().next().is_some())
}

/// The reference is the callee of a call standing alone as a statement (`Имя(...);`), whose
/// result nobody reads. Anything else — an assignment, an argument, a condition, an operand,
/// `Возврат Имя()`, the name without parentheses — reads the result, or is not understood
/// and counts as reading it.
fn is_call_statement(name: &SyntaxToken) -> bool {
    let Some(callee) = name.parent() else {
        return false;
    };
    let Some(call) = callee.parent().filter(|call| call.kind() == SyntaxKind::CALL_EXPR) else {
        return false;
    };
    call.first_child().as_ref() == Some(&callee)
        && call.parent().is_some_and(|stmt| stmt.kind() == SyntaxKind::CALL_STMT)
}

type KeywordPair = (&'static str, &'static str);

fn convert(
    file_id: FileId,
    method: &SyntaxNode,
    (open_kind, end_kind): (SyntaxKind, SyntaxKind),
    (russian, english): (KeywordPair, KeywordPair),
    id: AssistId,
    label: &str,
) -> Option<Assist> {
    let open = keyword(method, open_kind)?;
    let end = keyword(method, end_kind)?;
    let spelled = |token: &SyntaxToken, (ru, en): (&str, &str)| {
        if token.text().is_ascii() {
            en.to_string()
        } else {
            ru.to_string()
        }
    };
    let edits = vec![
        TextEdit { range: open.text_range(), new_text: spelled(&open, (russian.0, english.0)) },
        TextEdit { range: end.text_range(), new_text: spelled(&end, (russian.1, english.1)) },
    ];
    Some(Assist {
        id,
        label: label.to_string(),
        group: None,
        source_change: SourceChange { edits: vec![FileEdit { file_id, edits }] },
    })
}
