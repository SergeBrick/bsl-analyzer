use std::sync::Arc;
use stdx::case::CaseExt;

use bsl_types::kind::Projection;
use dataflow::reaching_defs::DefSite;
use hir_def::body::{Body, BodySourceMap};
use hir_def::hir::{Expr, ExprIdx, Literal, Stmt};
use hir_def::{DefWithBodyId, ExprId, IdConversion, MethodId, MethodIdInput, Name, StmtId};
use rustc_hash::FxHashSet;
use vfs::FileId;

use crate::db::HirDatabase;
use crate::infer::BodyInferenceResult;
use crate::method_lookup::{is_value_table_arm_id, projection_of_query_receiver_id};
use crate::sdbl_bridge::package_to_projections;

pub(crate) fn refine_query_at_use_site(
    db: &dyn HirDatabase,
    file_id: FileId,
    owner: DefWithBodyId,
    use_expr_id: ExprId,
    name: &Name,
    body: &Body,
) -> Option<Arc<[Option<Arc<Projection>>]>> {
    let DefWithBodyId::Method(local_id) = owner else {
        return None;
    };

    let dispatch_stmt_id = body.enclosing_stmt(use_expr_id)?;
    let literals = literal_text_defs(db, file_id, local_id, body, dispatch_stmt_id, name)?.literals;

    let mut candidate: Option<Arc<[Option<Arc<Projection>>]>> = None;
    for literal in literals {
        let projections = projections_of_literal(db, file_id, owner, literal)?;
        match &candidate {
            None => candidate = Some(projections),
            Some(prev) if projections_eq_ignoring_provenance(prev, &projections) => (),
            Some(_) => return None,
        }
    }
    candidate
}

struct LiteralTextDefs {
    literals: Vec<ExprId>,
    /// Definitions under both `Текст` and `Text` reach the statement. Reaching definitions
    /// keep the two spellings apart, so a write under one does not kill a write under the
    /// other, and the literals then include texts already replaced.
    both_spellings: bool,
}

/// The string literals assigned to `<receiver>.Текст` / `.Text` by every definition reaching
/// `stmt`. `None` when nothing reaches it or some definition is not such an assignment.
fn literal_text_defs(
    db: &dyn HirDatabase,
    file_id: FileId,
    local_id: hir_def::MethodKey,
    body: &Body,
    stmt: StmtId,
    receiver: &Name,
) -> Option<LiteralTextDefs> {
    let receiver_lower = receiver.as_str().fold_lower();
    let composite_ru = format!("{receiver_lower}.текст");
    let composite_en = format!("{receiver_lower}.text");

    let method_defs = db.method_reaching_definitions(MethodIdInput::new(
        db,
        hir_def::MethodId { module: hir_def::ModuleId::new(file_id), local_id },
    ))?;

    let mut defs = method_defs.defs_for_var_at_stmt(&composite_ru, stmt).unwrap_or_default();
    let ru_count = defs.len();
    if let Some(en_defs) = method_defs.defs_for_var_at_stmt(&composite_en, stmt) {
        defs.extend(en_defs);
    }
    if defs.is_empty() {
        return None;
    }
    let both_spellings = ru_count > 0 && defs.len() > ru_count;

    let literals = defs
        .into_iter()
        .map(|def| {
            let DefSite::Assignment(stmt_raw) = def.def_site else {
                return None;
            };
            let Stmt::Assign { value, .. } = body.stmt(StmtId::from_raw(stmt_raw)) else {
                return None;
            };
            let value_id = ExprId::from_idx(*value);
            matches!(body.expr(value_id), Expr::Literal(Literal::String(_))).then_some(value_id)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LiteralTextDefs { literals, both_spellings })
}

fn projections_eq_ignoring_provenance(
    a: &[Option<Arc<Projection>>],
    b: &[Option<Arc<Projection>>],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (None, None) => true,
            (Some(x), Some(y)) => {
                x.fields.len() == y.fields.len()
                    && x.raw_sdbl_types == y.raw_sdbl_types
                    && x.fields
                        .iter()
                        .zip(y.fields.iter())
                        .all(|(fx, fy)| fx.name == fy.name && fx.ty == fy.ty)
            }
            _ => false,
        })
}

fn projections_of_literal(
    db: &dyn HirDatabase,
    file_id: FileId,
    owner: DefWithBodyId,
    literal: ExprId,
) -> Option<Arc<[Option<Arc<Projection>>]>> {
    let pkg = hir_def::sdbl_package_for(db, file_id, owner, literal)?;

    if pkg.queries().is_empty() {
        return None;
    }
    Some(package_to_projections(db, &pkg).into())
}

/// A string literal that is the whole text of a `Запрос` object at a call executing it in this method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedQueryText {
    pub literal: ExprId,
    /// Parameters whose every binding on a receiver of that name in this method is a value
    /// table, when the method binds that receiver's parameters in no other way.
    pub value_table_parameters: Vec<Name>,
}

/// The literals that are the whole query text at an execution of a `Запрос` held in a plain
/// name. Definitions of `<name>.Текст` follow the name, not the object, so an execution has no
/// recovered text when the object is reachable under another name (handed to a call, assigned
/// elsewhere) or the name is rebound between the text and the call.
pub fn executed_query_texts(db: &dyn HirDatabase, method: MethodId) -> Vec<ExecutedQueryText> {
    let body = db.method_body_ref(MethodIdInput::new(db, method));
    let infer = db.infer_method(MethodIdInput::new(db, method));
    let lower = db.method_lower(MethodIdInput::new(db, method));
    let source_map = lower.as_deref().map(|lower| &lower.source_map);

    let mut texts: Vec<ExecutedQueryText> = Vec::new();
    for (call_id, expr) in body.exprs_iter() {
        let Some((receiver, called, _)) = as_method_call(body, expr) else {
            continue;
        };
        if !is_any_of(called, &["Выполнить", "Execute", "ВыполнитьПакет", "ExecuteBatch"])
        {
            continue;
        }
        let Expr::Path(query) = body.expr_idx(receiver) else {
            continue;
        };
        let is_query_object = infer
            .expr_types
            .get(&ExprId::from_idx(receiver))
            .is_some_and(|&ty| projection_of_query_receiver_id(db, ty).is_some());
        if !is_query_object {
            continue;
        }
        let Some(stmt) = body.enclosing_stmt(call_id) else {
            continue;
        };
        let Some(LiteralTextDefs { literals, both_spellings }) =
            literal_text_defs(db, method.module.file_id, method.local_id, body, stmt, query)
        else {
            continue;
        };
        if both_spellings
            || is_shared(body, query)
            || is_rebound_after_text(db, method, body, query, &literals, stmt)
        {
            continue;
        }

        let value_table_parameters =
            value_table_parameters(db, body, &infer, source_map, query, call_id);
        for literal in literals {
            if texts.iter().all(|text| text.literal != literal) {
                texts.push(ExecutedQueryText {
                    literal,
                    value_table_parameters: value_table_parameters.clone(),
                });
            }
        }
    }
    texts
}

/// `receiver.method(args)`. The body spells a method call as a call of a field.
fn as_method_call<'a>(
    body: &'a Body,
    expr: &'a Expr,
) -> Option<(ExprIdx, &'a Name, &'a [ExprIdx])> {
    match expr {
        Expr::MethodCall { receiver, method, args } => Some((*receiver, method, args)),
        Expr::Call { callee, args } => match body.expr_idx(*callee) {
            Expr::Field { base, field } => Some((*base, field, args)),
            _ => None,
        },
        _ => None,
    }
}

fn is_any_of(name: &Name, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| stdx::case::eq_ignore_case(name.as_str(), candidate))
}

fn is_path_named(body: &Body, expr: ExprIdx, name: &Name) -> bool {
    matches!(body.expr_idx(expr), Expr::Path(path) if stdx::case::eq_ignore_case(path.as_str(), name.as_str()))
}

/// The object in `query` may change under another name: some call takes `query` or its `Текст`
/// as an argument, or `query` is assigned to something else.
fn is_shared(body: &Body, query: &Name) -> bool {
    let assigned_elsewhere = body.stmts_iter().any(|(_, stmt)| {
        matches!(stmt, Stmt::Assign { value, .. } if is_path_named(body, *value, query))
    });
    assigned_elsewhere
        || body.exprs_iter().any(|(_, expr)| {
            let args = match expr {
                Expr::Call { args, .. }
                | Expr::MethodCall { args, .. }
                | Expr::New { args, .. } => args,
                _ => return false,
            };
            args.iter().any(|&arg| {
            is_path_named(body, arg, query)
                || matches!(
                    body.expr_idx(arg),
                    Expr::Field { base, field }
                        if is_path_named(body, *base, query) && is_any_of(field, &["Текст", "Text"])
                )
        })
        })
}

/// `query` names another object at `call` than at some assignment of a text literal: the
/// definitions of the name reaching the two statements differ. A definition of `<name>.Текст`
/// survives a rebinding of the name, so without this check a text written into a replaced
/// object would pass for the executed one.
fn is_rebound_after_text(
    db: &dyn HirDatabase,
    method: MethodId,
    body: &Body,
    query: &Name,
    literals: &[ExprId],
    call: StmtId,
) -> bool {
    let Some(method_defs) = db.method_reaching_definitions(MethodIdInput::new(db, method)) else {
        return true;
    };
    let query_lower = query.as_str().fold_lower();
    let defs_at = |stmt| -> Option<FxHashSet<_>> {
        Some(method_defs.defs_for_var_at_stmt(&query_lower, stmt)?.into_iter().collect())
    };
    let Some(at_call) = defs_at(call) else {
        return true;
    };
    literals.iter().any(|&literal| {
        body.enclosing_stmt(literal).and_then(defs_at).is_none_or(|at_text| at_text != at_call)
    })
}

/// Parameter names that every `query.УстановитьПараметр("<Name>", Value)` of the body binds to
/// a value table, at least one of them written before `execution`. Empty when the body binds
/// the parameters of `query` in another way: through `Параметры` or with a name that is not a
/// string literal.
fn value_table_parameters(
    db: &dyn HirDatabase,
    body: &Body,
    infer: &BodyInferenceResult,
    source_map: Option<&BodySourceMap>,
    query: &Name,
    execution: ExprId,
) -> Vec<Name> {
    let execution_start =
        source_map.and_then(|map| map.expr_range(execution)).map(|range| range.start());
    // A binding written only after the call leaves the parameter unset at it. Text order is
    // not control flow, so a binding before the call is required, not proven to reach it.
    let precedes_execution = |binding: ExprId| {
        let binding_start =
            source_map.and_then(|map| map.expr_range(binding)).map(|range| range.start());
        matches!((binding_start, execution_start), (Some(b), Some(e)) if b < e)
    };
    let mut bindings: Vec<(Name, bool, bool)> = Vec::new();
    for (binding_id, expr) in body.exprs_iter() {
        if let Expr::Field { base, field } = expr {
            if is_path_named(body, *base, query) && is_any_of(field, &["Параметры", "Parameters"])
            {
                return Vec::new();
            }
            continue;
        }
        let Some((receiver, method, args)) = as_method_call(body, expr) else {
            continue;
        };
        if !is_path_named(body, receiver, query)
            || !is_any_of(method, &["УстановитьПараметр", "SetParameter"])
        {
            continue;
        }
        let Some(&name_arg) = args.first() else {
            continue;
        };
        let Expr::Literal(Literal::String(name)) = body.expr_idx(name_arg) else {
            return Vec::new();
        };
        let Some(&value_arg) = args.get(1) else {
            continue;
        };
        let is_value_table = infer
            .expr_types
            .get(&ExprId::from_idx(value_arg))
            .is_some_and(|&ty| is_value_table_arm_id(db, ty));
        let precedes = precedes_execution(binding_id);
        match bindings
            .iter_mut()
            .find(|(bound, _, _)| stdx::case::eq_ignore_case(bound.as_str(), name))
        {
            Some((_, all_value_tables, any_precedes)) => {
                *all_value_tables &= is_value_table;
                *any_precedes |= precedes;
            }
            None => bindings.push((Name::from(name.as_str()), is_value_table, precedes)),
        }
    }
    bindings
        .into_iter()
        .filter(|(_, all_value_tables, any_precedes)| *all_value_tables && *any_precedes)
        .map(|(name, _, _)| name)
        .collect()
}
