use hir_def::execution_env::{self, EnvFlags, EnvOptions};
use hir_def::MethodId;

use crate::db::HirDatabase;

/// Environments a method body runs in: its module's, narrowed by its own
/// directive and by the module-level `#Если` regions around it. Read off the
/// position-free interface so a per-method query stays valid across edits.
pub(crate) fn effective_method_env(
    db: &dyn HirDatabase,
    method: MethodId,
    options: &EnvOptions,
) -> EnvFlags {
    let metadata = db.module_metadata(method.module);
    let Some(decl) = db.interface_method(method) else {
        return execution_env::body_env(&metadata, &[], options);
    };
    let environment = execution_env::body_env(&metadata, &decl.directives, options);
    if environment.is_empty() {
        return environment;
    }
    environment & decl.preproc_env
}

/// The module-level `#Если` regions around a method decidably rule out every
/// environment its module and directive give it, so the platform drops the
/// whole method. An unknown environment or an undecided region never counts.
pub(crate) fn method_compiled_nowhere(
    db: &dyn HirDatabase,
    method: MethodId,
    options: &EnvOptions,
) -> bool {
    let Some(decl) = db.interface_method(method) else {
        return false;
    };
    let metadata = db.module_metadata(method.module);
    let environment = execution_env::body_env(&metadata, &decl.directives, options);
    !environment.is_empty()
        && (environment & decl.preproc_env).is_empty()
        && (environment & decl.preproc_undecided).is_empty()
}
