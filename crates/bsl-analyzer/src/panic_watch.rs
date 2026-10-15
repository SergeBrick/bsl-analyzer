//! Which salsa revision a real panic happened in, so a propagated
//! *cancellation* is not reported to the client as one.
//!
//! Salsa hands a waiting thread [`salsa::Cancelled::PropagatedPanic`] whenever
//! the thread owning its query unwinds — a pending write included, because the
//! claim guard reserves `WaitResult::Cancelled` for a thread-LOCAL cancellation
//! only. The payload cannot tell the two apart, but the panic hook can: salsa
//! throws cancellations with `resume_unwind`, which deliberately skips the hook,
//! while a real panic always runs it.
//!
//! The revision is the whole point, and it is taken from salsa rather than
//! guessed. A panicking query poisons its memo for the revision it ran in, and
//! salsa replays that panic to later readers of the same revision (`execute.rs`,
//! `previous_iteration`) — readers that never blocked on anything. A reader of a
//! LATER revision is provably safe from it, so nothing narrower than the true
//! revision boundary may be used: an LRU eviction takes `&mut` on the database
//! without starting a revision, and a proxy that counted writes would drift
//! exactly there and hide the panic.
//!
//! A panic with no database attached to its thread cannot be replayed: nobody
//! can block on a thread that is not executing a query. Those are not stamped.

use std::any::Any;
use std::sync::Mutex;

use ide::RootDatabaseImpl;
use salsa::plumbing::{current_revision, with_attached_database};
use salsa::Revision;

static INSTALLED: std::sync::Once = std::sync::Once::new();

/// Chain a stamping hook in front of the current one.
///
/// Without it every propagated unwind reads as "nothing panicked", which would
/// hide real panics behind a retryable error code.
pub fn install() {
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Salsa attaches the database to a thread for the duration of a
            // tracked-fn body, which is exactly the window where a panic can
            // poison a memo or strand a waiter.
            with_attached_database(|attached| {
                // `Database` inherits `ZalsaDatabase: Any`, so this checked
                // downcast identifies our concrete storage without pointer ids
                // or unsafe Salsa view conversion.
                let Some(db) = (attached as &dyn Any).downcast_ref::<RootDatabaseImpl>() else {
                    return;
                };
                db.record_query_panic(current_revision(attached));
            });
            previous(info);
        }));
    });
}

/// Could a real panic from this storage reach a reader running in `revision`?
///
/// True for the same revision — poisoned-memo replay and stranded waiters alike
/// — and for anything newer, which is conservative by design.
pub fn panicked_in(db: &RootDatabaseImpl, revision: Revision) -> bool {
    db.query_panicked_in(revision)
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Clear one storage's stamp before an isolated test.
#[doc(hidden)]
pub fn reset_for_test(db: &RootDatabaseImpl) {
    db.reset_query_panic_for_test();
}

/// Serialises tests that panic on purpose against tests that assert no panic was
/// seen while exercising the process-wide panic hook.
#[doc(hidden)]
pub fn panic_test_guard() -> std::sync::MutexGuard<'static, ()> {
    lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Database as _;

    #[test]
    fn a_cancellation_is_not_a_panic() {
        let _guard = panic_test_guard();
        install();
        let db = RootDatabaseImpl::default();
        reset_for_test(&db);
        let revision = current_revision(&db);

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.attach(|_| std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite)))
        }));

        assert!(!panicked_in(&db, revision), "a cancellation unwinds past the hook");
    }

    #[test]
    fn a_panic_stamp_is_shared_by_clones_but_not_equal_revision_storages() {
        let _guard = panic_test_guard();
        install();
        let db = RootDatabaseImpl::default();
        let clone = db.clone();
        let independent = RootDatabaseImpl::default();
        let revision = current_revision(&db);
        assert_eq!(revision, current_revision(&independent));
        reset_for_test(&db);
        reset_for_test(&independent);

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.attach(|_| panic!("a query panicking"))
        }));

        assert!(panicked_in(&clone, revision), "clones share one Salsa storage");
        assert!(!panicked_in(&independent, revision), "independent storages stay isolated");
    }

    #[test]
    fn an_lru_eviction_is_not_a_revision_boundary_but_a_write_is() {
        let mut db = RootDatabaseImpl::default();
        let before = current_revision(&db);

        db.enforce_lru();
        assert_eq!(current_revision(&db), before, "LRU eviction must not start a revision");

        db.set_workspace_load_complete(true);
        assert!(current_revision(&db) > before, "an input write must start a revision");
    }

    #[test]
    fn a_panic_with_no_database_attached_is_not_stamped() {
        let _guard = panic_test_guard();
        install();
        let db = RootDatabaseImpl::default();
        reset_for_test(&db);
        let revision = current_revision(&db);

        let _ = std::panic::catch_unwind(|| panic!("a panic outside any query"));

        assert!(
            !panicked_in(&db, revision),
            "a thread not executing a query cannot strand a waiter or poison a memo"
        );
    }

    #[salsa::db]
    #[derive(Default)]
    struct OtherDatabase {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for OtherDatabase {}

    #[test]
    fn a_non_root_salsa_database_is_ignored_by_the_hook() {
        let _guard = panic_test_guard();
        install();
        let root = RootDatabaseImpl::default();
        let revision = current_revision(&root);
        reset_for_test(&root);
        let other = OtherDatabase::default();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            other.attach(|_| panic!("a panic in another Salsa database type"))
        }));

        assert!(!panicked_in(&root, revision));
    }
}
