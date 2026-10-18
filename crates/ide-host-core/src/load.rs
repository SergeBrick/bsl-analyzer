//! The disk-backed resident-load primitives shared by every consumer.
//!
//! A resident database that holds the whole workspace must NOT pin each file's text
//! as a salsa input (the overlay map lives outside the LRU and OOMs on a large
//! config). Instead each file is registered by its content revision and the text is
//! dropped; [`base_db::file_text_query`] re-reads it from disk on demand under its
//! LRU cap, verifying the bytes against the recorded revision. This is the model the
//! LSP server, the MCP diagnostics resident, and the CLI `analyze` pass all use, so
//! it lives here once.

use std::path::PathBuf;
use std::sync::Arc;

use base_db::{content_revision, read_disk_text, SourceDatabase, SourceRoot};
use ide::RootDatabaseImpl;
use vfs::file_set::FileSet;
use vfs::{FileId, VfsPath};

/// How a file's text is registered as a salsa input. The DRIVER decides which
/// variant (is the buffer open? was the file deleted?); this is the single place
/// that maps each intent to the right salsa mutation, so the "overlay vs disk-backed
/// vs tombstone" encoding — the exact distinction whose accidental conflation pins a
/// whole workspace's text resident — lives in one tunable spot.
pub enum FileTextSource<'a> {
    /// An open editor buffer is authoritative: its text is pinned as a resident
    /// overlay (`set_file_text`), source of truth for unsaved content.
    Overlay(&'a str),
    /// A closed / disk-backed file: registered by content revision only (the text is
    /// re-read on demand under the salsa LRU). The caller passes the text it already
    /// read; the revision is derived here so callers cannot diverge on the hash.
    Disk(&'a str),
    /// A deleted file: an empty overlay so a later query yields `""` instead of
    /// panicking on a disk re-read. FileSet removal (if any) stays with the driver —
    /// this only sets the text state.
    Deleted,
    /// A file that is still there but whose bytes could not be read: the same empty
    /// overlay, plus the mark that says the emptiness is ignorance.
    ///
    /// Split from [`Deleted`](Self::Deleted) because one variant cannot carry two
    /// meanings and still be written down: name resolution has to tell "this module
    /// has no API" from "nobody could read this module's API", and both arrive here
    /// as the same empty text.
    Unreadable,
}

/// Owned payload for one item in a source batch. LSP changes retain the VFS
/// `Arc<str>`; disk readers can pass their one-file `String` through without
/// collecting the workspace text first.
pub enum BatchTextSource {
    /// Open editor content, stored as a resident overlay.
    Overlay(Arc<str>),
    /// Closed-file content, represented by its revision and read on demand.
    Disk(Arc<str>),
    /// The same disk-backed source for a caller that owns a one-file read.
    DiskOwned(String),
    /// A prepared disk revision copied from another Salsa storage.
    DiskRevision(u64),
    /// A deleted file; retain an empty readable tombstone.
    Deleted,
    /// A present file whose bytes could not be read.
    Unreadable,
}

/// One ordered file-universe or source-input mutation in a prepared batch.
pub enum SourceBatchChange {
    /// Insert or update a source path. Missing source is reserved for non-BSL service files.
    Upsert {
        /// The VFS identity used by Salsa and the FileSet.
        file_id: FileId,
        /// The path published in the source root.
        path: VfsPath,
        /// Source input, omitted only for non-BSL service entries.
        source: Option<BatchTextSource>,
    },
    /// Remove a registered file from the source root and tombstone BSL content.
    Remove {
        /// The VFS identity to remove.
        file_id: FileId,
    },
}

/// Observable source changes produced by applying a batch.
#[derive(Debug, Default)]
pub struct SourceBatchEffects {
    /// Whether the published FileSet differs from the input FileSet.
    pub file_set_changed: bool,
    /// Whether BSL membership or content changed.
    pub bsl_source_changed: bool,
    /// IDs whose path or source inputs actually changed.
    pub changed_file_ids: Vec<FileId>,
    /// Paths associated with actual source or membership changes.
    pub affected_paths: Vec<PathBuf>,
}

/// Apply a prepared source batch to one existing root. Drivers retain path
/// policy, metadata lifecycle, and admission decisions; this function only
/// orders file inputs before the single FileSet publication.
pub fn apply_source_batch(
    db: &mut RootDatabaseImpl,
    source_root_id: base_db::SourceRootId,
    changes: impl IntoIterator<Item = SourceBatchChange>,
) -> SourceBatchEffects {
    apply_source_batch_cancellable(db, source_root_id, changes, || false)
        .expect("a non-cancellable source batch cannot be cancelled")
}

pub(crate) fn apply_source_batch_cancellable(
    db: &mut RootDatabaseImpl,
    source_root_id: base_db::SourceRootId,
    changes: impl IntoIterator<Item = SourceBatchChange>,
    mut cancelled: impl FnMut() -> bool,
) -> Option<SourceBatchEffects> {
    use base_db::{SourceDatabase, SourceRoot};

    let mut changes = changes.into_iter().peekable();
    if changes.peek().is_none() {
        return Some(SourceBatchEffects::default());
    }

    // Finish the read and drop the Salsa value before the first tracked write.
    let mut file_set = {
        let source_root = db.source_root_input(source_root_id).root(db);
        source_root.file_set().clone()
    };
    let mut effects = SourceBatchEffects::default();

    for change in changes {
        if cancelled() {
            return None;
        }
        match change {
            SourceBatchChange::Upsert { file_id, path, source } => {
                let is_bsl = project_model::is_bsl_source_path(path.as_path());
                assert!(source.is_some() || !is_bsl, "visible BSL sources need a content revision");
                let path_changed = file_set.path_for_file(&file_id) != Some(&path);
                let (source_changed, disk_revision) =
                    source.as_ref().map_or((false, None), |source| {
                        let (matches, revision) = source_matches(db, file_id, source);
                        (!matches, revision)
                    });
                if !path_changed && !source_changed {
                    continue;
                }

                db.set_file_source_root(file_id, source_root_id);
                if let Some(source) = source {
                    apply_owned_source(db, file_id, source, disk_revision);
                }
                if path_changed {
                    file_set.remove(file_id);
                    file_set.insert(file_id, path.clone());
                    effects.file_set_changed = true;
                }
                if source_changed || path_changed {
                    effects.changed_file_ids.push(file_id);
                    effects.affected_paths.push(path.as_path().to_path_buf());
                    effects.bsl_source_changed |= is_bsl;
                }
            }
            SourceBatchChange::Remove { file_id } => {
                let Some(path) = file_set.path_for_file(&file_id).cloned() else {
                    continue;
                };
                let is_bsl = project_model::is_bsl_source_path(path.as_path());
                if is_bsl {
                    let source = BatchTextSource::Deleted;
                    let (matches, disk_revision) = source_matches(db, file_id, &source);
                    if !matches {
                        apply_owned_source(db, file_id, source, disk_revision);
                    }
                }
                file_set.remove(file_id);
                effects.file_set_changed = true;
                effects.changed_file_ids.push(file_id);
                effects.affected_paths.push(path.as_path().to_path_buf());
                effects.bsl_source_changed |= is_bsl;
            }
        }
    }

    if cancelled() {
        return None;
    }
    if effects.file_set_changed {
        db.set_source_root(source_root_id, SourceRoot::new_local(file_set));
    }
    Some(effects)
}

fn source_matches(
    db: &RootDatabaseImpl,
    file_id: FileId,
    source: &BatchTextSource,
) -> (bool, Option<u64>) {
    use base_db::SourceDatabase;

    let disk_revision = match source {
        BatchTextSource::Disk(text) => Some(content_revision(text)),
        BatchTextSource::DiskOwned(text) => Some(content_revision(text)),
        BatchTextSource::DiskRevision(revision) => Some(*revision),
        _ => None,
    };
    let Some(revision_input) = db.try_file_revision_input(file_id) else {
        return (false, disk_revision);
    };
    let unreadable = matches!(source, BatchTextSource::Unreadable);
    if revision_input.unreadable(db) != unreadable {
        return (false, disk_revision);
    }

    let matches = match source {
        BatchTextSource::Overlay(text) => {
            let Some(input) = db.try_file_text_input(file_id) else { return (false, None) };
            if input.text(db) != text.as_ref() {
                return (false, None);
            }
            revision_input.revision(db) == content_revision(text)
        }
        BatchTextSource::Disk(_)
        | BatchTextSource::DiskOwned(_)
        | BatchTextSource::DiskRevision(_) => {
            db.try_file_text_input(file_id).is_none()
                && revision_input.revision(db)
                    == disk_revision.expect("disk source has a precomputed revision")
        }
        BatchTextSource::Deleted | BatchTextSource::Unreadable => {
            db.try_file_text_input(file_id).is_some_and(|input| input.text(db).is_empty())
                && revision_input.revision(db) == content_revision("")
        }
    };
    (matches, disk_revision)
}

fn apply_owned_source(
    db: &mut RootDatabaseImpl,
    file_id: FileId,
    source: BatchTextSource,
    disk_revision: Option<u64>,
) {
    match source {
        BatchTextSource::Overlay(text) => {
            set_file_text_source(db, file_id, FileTextSource::Overlay(&text))
        }
        BatchTextSource::Disk(_)
        | BatchTextSource::DiskOwned(_)
        | BatchTextSource::DiskRevision(_) => db.set_file_revision_from_disk(
            file_id,
            disk_revision.expect("disk source revision was computed during comparison"),
        ),
        BatchTextSource::Deleted => set_file_text_source(db, file_id, FileTextSource::Deleted),
        BatchTextSource::Unreadable => {
            set_file_text_source(db, file_id, FileTextSource::Unreadable)
        }
    }
}

/// Apply one file's [`FileTextSource`] to `db`. A leaf salsa mutation only: it does
/// not touch the source root / FileSet, project config, or any driver lifecycle.
pub fn set_file_text_source(
    db: &mut RootDatabaseImpl,
    file_id: FileId,
    source: FileTextSource<'_>,
) {
    match source {
        FileTextSource::Overlay(text) => db.set_file_text(file_id, text),
        FileTextSource::Disk(text) => {
            db.set_file_revision_from_disk(file_id, content_revision(text))
        }
        FileTextSource::Deleted => db.set_file_text(file_id, ""),
        FileTextSource::Unreadable => db.set_file_unreadable(file_id),
    }
}

/// Build a whole-workspace [`SourceRoot`]: a `FileId ↔ path` map over EVERY file, so
/// cross-module resolution through the module index can find any target's `FileId`.
/// Cheap to clone (the map is `Arc`-backed).
pub fn build_source_root(all_files: &[(FileId, PathBuf)]) -> SourceRoot {
    let mut file_set = FileSet::new();
    for (file_id, path) in all_files {
        file_set.insert(*file_id, VfsPath::new(path.clone()));
    }
    SourceRoot::new_local(file_set)
}

/// Register every file into `db` under `source_root_id` as a disk-backed content
/// revision — the text is read once to hash it, then dropped, so it is re-read on
/// demand under the salsa LRU rather than pinned resident.
///
/// `file_source_root` is set for every file because [`base_db::file_text_query`]
/// derives the on-disk path through it. An unreadable file is registered as an empty
/// overlay (so a later query yields `""` instead of panicking on the disk re-read)
/// and its `(path, error)` is returned, letting a strict caller (e.g. CLI) surface
/// the real read error while a lenient caller (e.g. the MCP resident) keeps serving.
/// The existing root is updated through [`apply_source_batch`], so new file inputs
/// are installed before a changed FileSet is published and unrelated paths survive.
pub fn register_files_disk_backed(
    db: &mut RootDatabaseImpl,
    source_root_id: base_db::SourceRootId,
    files: &[(FileId, PathBuf)],
) -> Vec<(PathBuf, std::io::Error)> {
    let mut unreadable = Vec::new();
    let changes = files.iter().map(|(file_id, path)| {
        let source = match read_disk_text(path) {
            Ok(text) => BatchTextSource::DiskOwned(text),
            Err(error) => {
                tracing::warn!(path = %path.display(), "resident load: read failed: {error}");
                unreadable.push((path.clone(), error));
                BatchTextSource::Unreadable
            }
        };
        SourceBatchChange::Upsert {
            file_id: *file_id,
            path: VfsPath::new(path.clone()),
            source: Some(source),
        }
    });
    apply_source_batch(db, source_root_id, changes);
    unreadable
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide::Analysis;

    fn empty_root_db() -> RootDatabaseImpl {
        use base_db::SourceDatabase;

        let mut db = RootDatabaseImpl::default();
        db.set_source_root(base_db::SourceRootId(0), SourceRoot::new_local(FileSet::new()));
        db
    }

    fn upsert(file_id: FileId, path: &str, source: BatchTextSource) -> SourceBatchChange {
        SourceBatchChange::Upsert {
            file_id,
            path: VfsPath::new(PathBuf::from(path)),
            source: Some(source),
        }
    }

    #[test]
    fn source_batch_registers_inputs_before_visibility_and_never_creates_root_one() {
        use base_db::SourceDatabase;

        let mut db = empty_root_db();
        let file_id = FileId(7);
        let path = VfsPath::new(PathBuf::from("/workspace/a.bsl"));
        let effects = apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(
                file_id,
                "/workspace/a.bsl",
                BatchTextSource::Disk(Arc::from("Процедура A() КонецПроцедуры")),
            )],
        );

        assert!(effects.file_set_changed);
        assert!(effects.bsl_source_changed);
        assert!(db.try_file_revision_input(file_id).is_some());
        assert_eq!(
            db.file_source_root_input(file_id).source_root_id(&db),
            base_db::SourceRootId(0)
        );
        assert_eq!(
            db.source_root_input(base_db::SourceRootId(0))
                .root(&db)
                .file_set()
                .path_for_file(&file_id),
            Some(&path)
        );
        assert!(db.try_source_root_input(base_db::SourceRootId(1)).is_none());
    }

    #[test]
    fn source_batch_repeated_inputs_are_a_noop_and_same_id_can_be_recreated() {
        use base_db::SourceDatabase;

        let mut db = empty_root_db();
        let file_id = FileId(3);
        let add = || {
            upsert(
                file_id,
                "/workspace/recreated.bsl",
                BatchTextSource::Overlay(Arc::from("Текст")),
            )
        };
        apply_source_batch(&mut db, base_db::SourceRootId(0), [add()]);
        let unchanged = apply_source_batch(&mut db, base_db::SourceRootId(0), [add()]);
        assert!(unchanged.changed_file_ids.is_empty());
        assert!(!unchanged.file_set_changed);

        apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [SourceBatchChange::Remove { file_id }],
        );
        let recreated = apply_source_batch(&mut db, base_db::SourceRootId(0), [add()]);
        assert!(recreated.file_set_changed);
        assert_eq!(
            db.source_root_input(base_db::SourceRootId(0))
                .root(&db)
                .file_set()
                .path_for_file(&file_id),
            Some(&VfsPath::new(PathBuf::from("/workspace/recreated.bsl")))
        );
        assert_eq!(&*Analysis::from_database(db.clone()).file_text(file_id), "Текст");
    }

    #[test]
    fn source_batch_overlay_change_and_empty_overlay_remain_distinct_from_unreadable() {
        use base_db::SourceDatabase;

        let mut db = empty_root_db();
        let file_id = FileId(11);
        let path = "/workspace/overlay.bsl";
        let apply_overlay = |db: &mut RootDatabaseImpl, text: &str| {
            apply_source_batch(
                db,
                base_db::SourceRootId(0),
                [upsert(file_id, path, BatchTextSource::Overlay(Arc::from(text)))],
            )
        };

        apply_overlay(&mut db, "");
        let unchanged = apply_overlay(&mut db, "");
        assert!(unchanged.changed_file_ids.is_empty());
        assert!(!db.file_is_unread(file_id));

        let changed = apply_overlay(&mut db, "changed");
        assert_eq!(changed.changed_file_ids.as_slice(), &[file_id]);
        assert_eq!(&*Analysis::from_database(db.clone()).file_text(file_id), "changed");

        let unreadable = apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Unreadable)],
        );
        assert_eq!(unreadable.changed_file_ids.as_slice(), &[file_id]);
        assert!(db.file_is_unread(file_id));

        let still_unreadable = apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Unreadable)],
        );
        assert!(still_unreadable.changed_file_ids.is_empty());

        let readable_empty = apply_overlay(&mut db, "");
        assert_eq!(readable_empty.changed_file_ids.as_slice(), &[file_id]);
        assert!(!db.file_is_unread(file_id));
    }

    #[test]
    fn source_batch_distinguishes_overlay_disk_deleted_and_unreadable() {
        use base_db::SourceDatabase;

        let mut db = empty_root_db();
        let file_id = FileId(9);
        let path = "/workspace/modes.bsl";
        apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Overlay(Arc::from("same")))],
        );
        let disk = apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Disk(Arc::from("same")))],
        );
        assert!(disk.bsl_source_changed, "overlay-to-disk is a source-mode change");
        assert!(db.try_file_text_input(file_id).is_none());

        apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Unreadable)],
        );
        assert!(db.file_is_unread(file_id));
        apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [SourceBatchChange::Remove { file_id }],
        );
        assert!(!db.file_is_unread(file_id), "deletion clears unreadable state");
    }

    #[test]
    fn source_batch_disk_owned_noop_and_unreadable_healing_remain_disk_backed() {
        use base_db::SourceDatabase;

        let mut db = empty_root_db();
        let file_id = FileId(13);
        let path = "/workspace/owned-disk.bsl";
        let apply_disk = |db: &mut RootDatabaseImpl, text: &str| {
            apply_source_batch(
                db,
                base_db::SourceRootId(0),
                [upsert(file_id, path, BatchTextSource::DiskOwned(text.to_owned()))],
            )
        };

        let initial = apply_disk(&mut db, "initial");
        assert_eq!(initial.changed_file_ids.as_slice(), &[file_id]);
        assert!(db.try_file_text_input(file_id).is_none(), "disk text is not pinned");
        assert_eq!(
            db.try_file_revision_input(file_id).unwrap().revision(&db),
            content_revision("initial")
        );

        let unchanged = apply_disk(&mut db, "initial");
        assert!(unchanged.changed_file_ids.is_empty());
        assert!(!unchanged.file_set_changed);
        assert!(db.try_file_text_input(file_id).is_none(), "no-op keeps disk text unpinned");
        assert_eq!(
            db.try_file_revision_input(file_id).unwrap().revision(&db),
            content_revision("initial")
        );

        let unreadable = apply_source_batch(
            &mut db,
            base_db::SourceRootId(0),
            [upsert(file_id, path, BatchTextSource::Unreadable)],
        );
        assert_eq!(unreadable.changed_file_ids.as_slice(), &[file_id]);
        assert!(db.file_is_unread(file_id));

        let healed = apply_disk(&mut db, "recovered");
        assert_eq!(healed.changed_file_ids.as_slice(), &[file_id]);
        assert!(!db.file_is_unread(file_id));
        assert!(db.try_file_text_input(file_id).is_none(), "healed disk text is not pinned");
        assert_eq!(
            db.try_file_revision_input(file_id).unwrap().revision(&db),
            content_revision("recovered")
        );
    }

    #[test]
    fn source_batch_consumes_owned_text_one_item_at_a_time() {
        use std::sync::Weak;

        let mut db = empty_root_db();
        let mut next = 0u32;
        let mut previous: Option<Weak<str>> = None;
        let stream = std::iter::from_fn(|| {
            if let Some(previous) = previous.take() {
                assert!(previous.upgrade().is_none(), "previous batch text is still retained");
            }
            if next == 8 {
                return None;
            }
            let file_id = FileId(next);
            let path = format!("/workspace/{next}.bsl");
            let text: Arc<str> = Arc::from(format!("text {next}"));
            previous = Some(Arc::downgrade(&text));
            next += 1;
            Some(upsert(file_id, &path, BatchTextSource::Disk(text)))
        });

        let effects = apply_source_batch(&mut db, base_db::SourceRootId(0), stream);
        assert_eq!(next, 8);
        assert_eq!(effects.changed_file_ids.len(), 8);
    }

    #[test]
    fn disk_backed_registration_and_unreadable_report() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("a.bsl");
        std::fs::write(&good, "Процедура П() КонецПроцедуры").unwrap();
        let missing = dir.path().join("missing.bsl");

        let files = vec![(FileId(0), good.clone()), (FileId(1), missing.clone())];
        let root = base_db::SourceRootId(0);
        let mut db = RootDatabaseImpl::default();
        db.set_source_root(root, SourceRoot::new_local(FileSet::new()));
        let unreadable = register_files_disk_backed(&mut db, root, &files);

        // The missing file is reported with its read error (and overlaid empty); the
        // readable file is disk-backed — no `FileTextInput` overlay pinned — yet queryable.
        assert_eq!(unreadable.len(), 1);
        assert_eq!(unreadable[0].0, missing);
        let source_root = db.source_root_input(root).root(&db);
        assert_eq!(source_root.file_set().path_for_file(&FileId(0)), Some(&VfsPath::new(good)));
        assert_eq!(source_root.file_set().path_for_file(&FileId(1)), Some(&VfsPath::new(missing)));
        assert_eq!(db.file_source_root_input(FileId(0)).source_root_id(&db), root);
        assert_eq!(db.file_source_root_input(FileId(1)).source_root_id(&db), root);
        assert!(db.try_file_text(FileId(0)).is_none(), "readable file must be disk-backed");
        assert!(db.file_is_unread(FileId(1)));
        let text = Analysis::from_database(db.clone()).file_text(FileId(0));
        assert!(text.contains("Процедура"), "disk-backed text is read on demand");
    }

    #[test]
    fn file_text_source_variants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bsl");
        std::fs::write(&path, "Текст").unwrap();
        let files = vec![(FileId(0), path)];
        let root = base_db::SourceRootId(0);
        let mut db = RootDatabaseImpl::default();
        db.set_source_root(root, build_source_root(&files));
        db.set_file_source_root(FileId(0), root);

        let read = |db: &RootDatabaseImpl| Analysis::from_database(db.clone()).file_text(FileId(0));

        // Overlay: editor buffer pinned as a resident input.
        set_file_text_source(&mut db, FileId(0), FileTextSource::Overlay("буфер"));
        assert!(db.try_file_text(FileId(0)).is_some(), "overlay is pinned");
        assert_eq!(&*read(&db), "буфер");

        // Disk: overlay dropped, text re-read from disk and verified against the revision.
        set_file_text_source(&mut db, FileId(0), FileTextSource::Disk("Текст"));
        assert!(db.try_file_text(FileId(0)).is_none(), "disk-backed is not pinned");
        assert_eq!(&*read(&db), "Текст");

        // Deleted: empty overlay, and the emptiness stands for content.
        set_file_text_source(&mut db, FileId(0), FileTextSource::Deleted);
        assert_eq!(&*read(&db), "");
        assert!(!db.file_is_unread(FileId(0)), "a deleted file is not an unread one");

        // Unreadable: the same empty overlay, marked.
        set_file_text_source(&mut db, FileId(0), FileTextSource::Unreadable);
        assert_eq!(&*read(&db), "");
        assert!(db.file_is_unread(FileId(0)));
    }
}
