//! Remembers which debugger UI id this user's session registered for a debug endpoint.
//!
//! The debug server lets one debugger UI hold an infobase at a time and answers every other
//! `attachDebugUI` with `ibInDebug`. A session that dies without `detachDebugUI` (its process was
//! killed) keeps holding the infobase until the server notices about a minute of silence on its
//! event channel; meanwhile a target it had halted stays halted, and once the server drops the
//! registration that target's client has been seen to stay frozen for good. The server accepts a
//! re-registration under the SAME id at once, though, so a later session that knows the id can
//! take the dead session's place and resume what it left halted.
//!
//! The file holding the id is locked for the whole life of the session that wrote it. The OS
//! releases that lock when the process dies, so "the file is lockable" is exactly "its writer is
//! gone" — the id of a session that is still alive is never reused.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct Lease {
    file: File,
    path: PathBuf,
}

pub enum Reclaim {
    /// The writer is gone; this lease now belongs to the caller.
    Reclaimed { lease: Lease, debugger_id: String },
    /// Another live session of this user holds the endpoint.
    HeldByLiveSession,
    /// No session of this user left an id for this endpoint.
    NoLease,
}

impl Lease {
    /// One file per endpoint: the server keys a debugger registration by infobase alias, and the
    /// same alias on two debug servers is two infobases.
    pub fn file_name(host: &str, port: u16, infobase: &str) -> String {
        let raw = format!("{host}_{port}_{infobase}");
        let safe: String = raw
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        format!("{safe}.lease")
    }

    /// Records `debugger_id` for the endpoint and holds the file locked. `Ok(None)` when another
    /// live session holds the lease: its id must not be overwritten while it may still be used.
    pub fn acquire(
        dir: &Path,
        file_name: &str,
        debugger_id: &str,
    ) -> std::io::Result<Option<Self>> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(file_name);
        let mut file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(debugger_id.as_bytes())?;
        file.flush()?;
        Ok(Some(Self { file, path }))
    }

    pub fn reclaim(dir: &Path, file_name: &str) -> std::io::Result<Reclaim> {
        let path = dir.join(file_name);
        let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Reclaim::NoLease),
            Err(e) => return Err(e),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(Reclaim::HeldByLiveSession),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let debugger_id = text.trim().to_string();
        let lease = Self { file, path };
        if debugger_id.is_empty() {
            lease.release();
            return Ok(Reclaim::NoLease);
        }
        Ok(Reclaim::Reclaimed { lease, debugger_id })
    }

    /// Called only after `detachDebugUI` succeeded: nothing is left registered under the id, so
    /// there is nothing a later session could take over.
    pub fn release(self) {
        let Self { file, path } = self;
        let _ = file.unlock();
        drop(file);
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::debug!(path = %path.display(), error = %e, "debugger lease not removed");
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "localhost_1550_DefAlias.lease";

    #[test]
    fn a_lease_of_a_live_session_is_not_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let held = Lease::acquire(dir.path(), NAME, "id-1").unwrap().expect("first lease");
        assert!(matches!(Lease::reclaim(dir.path(), NAME).unwrap(), Reclaim::HeldByLiveSession));
        assert!(
            Lease::acquire(dir.path(), NAME, "id-2").unwrap().is_none(),
            "a live session's id must not be overwritten"
        );
        drop(held);
    }

    #[test]
    fn a_lease_left_by_a_dead_session_hands_its_id_over() {
        let dir = tempfile::tempdir().unwrap();
        // Dropping without `release` is what a killed process leaves behind: the file stays,
        // the lock goes with the handle.
        drop(Lease::acquire(dir.path(), NAME, "id-dead").unwrap().expect("lease"));
        match Lease::reclaim(dir.path(), NAME).unwrap() {
            Reclaim::Reclaimed { lease, debugger_id } => {
                assert_eq!(debugger_id, "id-dead");
                assert!(
                    matches!(Lease::reclaim(dir.path(), NAME).unwrap(), Reclaim::HeldByLiveSession),
                    "the reclaimed lease is held by its new owner"
                );
                lease.release();
            }
            _ => panic!("the dead session's id must be handed over"),
        }
        assert!(matches!(Lease::reclaim(dir.path(), NAME).unwrap(), Reclaim::NoLease));
    }

    #[test]
    fn a_released_lease_leaves_nothing_to_reclaim() {
        let dir = tempfile::tempdir().unwrap();
        Lease::acquire(dir.path(), NAME, "id-1").unwrap().expect("lease").release();
        assert!(!dir.path().join(NAME).exists(), "release removes the file");
        assert!(matches!(Lease::reclaim(dir.path(), NAME).unwrap(), Reclaim::NoLease));
    }

    #[test]
    fn a_shorter_id_overwrites_a_longer_one_completely() {
        let dir = tempfile::tempdir().unwrap();
        drop(Lease::acquire(dir.path(), NAME, "a-much-longer-identifier").unwrap());
        drop(Lease::acquire(dir.path(), NAME, "short").unwrap());
        match Lease::reclaim(dir.path(), NAME).unwrap() {
            Reclaim::Reclaimed { debugger_id, .. } => assert_eq!(debugger_id, "short"),
            _ => panic!("expected the last id"),
        }
    }

    #[test]
    fn the_file_name_keeps_the_alias_readable_and_the_path_safe() {
        assert_eq!(Lease::file_name("localhost", 1550, "DefAlias"), NAME);
        assert_eq!(Lease::file_name("10.0.0.5", 1550, "база/1"), "10.0.0.5_1550_база_1.lease");
    }
}
