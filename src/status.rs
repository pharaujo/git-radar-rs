//! Working tree changes, counted the way git-radar 0.6 counts `git status --porcelain` lines.
//!
//! Each path gets the porcelain-v1 `XY` pair that git would print. Pairs are then counted
//! against the exact `grep -oE` patterns from radar-base.sh (see [`Counts::add`]). Counting
//! per column instead would differ, e.g. on typechanges (`MT`) or intent-to-add (` A`).

use std::collections::BTreeMap;

use gix::bstr::{BString, ByteSlice};
use gix::status::index_worktree::Item as WtItem;
use gix::status::plumbing::index_as_worktree::{Change, Conflict, EntryStatus};

/// Per-bucket counts, in git-radar's terms.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub staged_added: usize,
    pub staged_deleted: usize,
    pub staged_modified: usize,
    pub staged_renamed: usize,
    pub staged_copied: usize,
    pub conflict_us: usize,
    pub conflict_them: usize,
    pub conflict_both: usize,
    pub unstaged_deleted: usize,
    pub unstaged_modified: usize,
    pub untracked: usize,
}

impl Counts {
    /// Count one porcelain `XY` pair using git-radar's patterns:
    ///
    /// | bucket          | pattern       |
    /// |-----------------|---------------|
    /// | staged A        | `A[MCDR ]`    |
    /// | staged D        | `D[AMCR ]`    |
    /// | staged M        | `M[ACDRM ]`   |
    /// | staged R        | `R[AMCD ]`    |
    /// | staged C        | `C[AMDR ]`    |
    /// | unstaged D      | `[AMCR ]D`    |
    /// | unstaged M      | `[ACDRM ]M`   |
    /// | conflict us     | `[AD]U`       |
    /// | conflict them   | `U[AD]`       |
    /// | conflict both   | `UU\|AA\|DD`  |
    /// | untracked       | `??`          |
    pub fn add(&mut self, x: u8, y: u8) {
        match (x, y) {
            (b'A', b'M' | b'C' | b'D' | b'R' | b' ') => self.staged_added += 1,
            (b'D', b'A' | b'M' | b'C' | b'R' | b' ') => self.staged_deleted += 1,
            (b'M', b'A' | b'C' | b'D' | b'R' | b'M' | b' ') => self.staged_modified += 1,
            (b'R', b'A' | b'M' | b'C' | b'D' | b' ') => self.staged_renamed += 1,
            (b'C', b'A' | b'M' | b'D' | b'R' | b' ') => self.staged_copied += 1,
            _ => {}
        }
        match (x, y) {
            (b'A' | b'M' | b'C' | b'R' | b' ', b'D') => self.unstaged_deleted += 1,
            (b'A' | b'C' | b'D' | b'R' | b'M' | b' ', b'M') => self.unstaged_modified += 1,
            _ => {}
        }
        match (x, y) {
            (b'A' | b'D', b'U') => self.conflict_us += 1,
            (b'U', b'A' | b'D') => self.conflict_them += 1,
            (b'U', b'U') | (b'A', b'A') | (b'D', b'D') => self.conflict_both += 1,
            (b'?', b'?') => self.untracked += 1,
            _ => {}
        }
    }
}

/// Porcelain status letters per path, before counting.
#[derive(Debug, Default, Clone)]
pub struct Entries {
    /// path → (X, Y). `?` `?` for untracked entries.
    pub xy: BTreeMap<BString, (u8, u8)>,
}

impl Entries {
    fn set_x(&mut self, path: BString, x: u8) {
        self.xy.entry(path).or_insert((b' ', b' ')).0 = x;
    }
    fn set_y(&mut self, path: BString, y: u8) {
        self.xy.entry(path).or_insert((b' ', b' ')).1 = y;
    }
    fn set_both(&mut self, path: BString, x: u8, y: u8) {
        self.xy.insert(path, (x, y));
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for &(x, y) in self.xy.values() {
            c.add(x, y);
        }
        c
    }

    pub fn has_tracked_changes(&self) -> bool {
        self.xy.values().any(|&xy| xy != (b'?', b'?'))
    }

    pub fn has_untracked(&self) -> bool {
        self.xy.values().any(|&xy| xy == (b'?', b'?'))
    }
}

/// Mode "kind" as git uses it to tell a typechange (`T`) from a modification (`M`). Only an
/// executable-bit flip stays within a kind.
fn mode_kind(mode: gix::index::entry::Mode) -> u8 {
    use gix::index::entry::Mode;
    if mode == Mode::FILE || mode == Mode::FILE_EXECUTABLE {
        0
    } else if mode == Mode::SYMLINK {
        1
    } else if mode == Mode::COMMIT {
        2
    } else {
        3
    }
}

/// The letter for a HEAD-tree ↔ index change.
pub fn tree_index_letter(change: &gix::diff::index::Change) -> (BString, u8) {
    use gix::diff::index::Change as C;
    match change {
        C::Addition { location, .. } => (location.as_ref().to_owned(), b'A'),
        C::Deletion { location, .. } => (location.as_ref().to_owned(), b'D'),
        C::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            let letter = if mode_kind(*previous_entry_mode) != mode_kind(*entry_mode) {
                b'T'
            } else {
                b'M'
            };
            (location.as_ref().to_owned(), letter)
        }
        C::Rewrite { location, copy, .. } => (
            location.as_ref().to_owned(),
            if *copy { b'C' } else { b'R' },
        ),
    }
}

/// Porcelain codes for an unmerged path.
pub fn conflict_letters(conflict: Conflict) -> (u8, u8) {
    match conflict {
        Conflict::BothDeleted => (b'D', b'D'),
        Conflict::AddedByUs => (b'A', b'U'),
        Conflict::DeletedByThem => (b'U', b'D'),
        Conflict::AddedByThem => (b'U', b'A'),
        Conflict::DeletedByUs => (b'D', b'U'),
        Conflict::BothAdded => (b'A', b'A'),
        Conflict::BothModified => (b'U', b'U'),
    }
}

/// The worktree letter for an index ↔ worktree change. `None` means clean. Exhaustive on purpose,
/// so a new gix variant breaks the build instead of quietly turning into `M`.
pub fn worktree_letter<T, U>(status: &EntryStatus<T, U>) -> Option<u8> {
    match status {
        EntryStatus::Conflict { .. } => None, // handled separately
        EntryStatus::Change(change) => Some(match change {
            Change::Removed => b'D',
            Change::Type { .. } => b'T',
            Change::Modification { .. } => b'M',
            Change::SubmoduleModification(_) => b'M',
        }),
        EntryStatus::NeedsUpdate(_) => None,
        EntryStatus::IntentToAdd => Some(b'A'),
    }
}

/// Remove every `[filter "<name>"]` section from the in-memory config so status never runs
/// a clean filter (e.g. `git lfs filter-process`). Only the built-in eol/ident conversions remain.
pub fn strip_filter_drivers(repo: &mut gix::Repository) {
    let mut snapshot = repo.config_snapshot_mut();
    let ids: Vec<_> = snapshot
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        .map(|(_, id)| id)
        .collect();
    if ids.is_empty() {
        snapshot.forget();
        return;
    }
    for id in ids {
        snapshot.remove_section_by_id(id);
    }
    // Committing only fails on invalid values, and we only removed sections.
    let _ = snapshot.commit();
}

/// How much of a submodule's state counts as a change, per `submodule.<name>.ignore` or
/// `diff.ignoreSubmodules`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SubmoduleIgnore {
    None,
    Untracked,
    Dirty,
    All,
}

fn submodule_ignore(repo: &gix::Repository, rela_path: &[u8]) -> SubmoduleIgnore {
    use gix::submodule::config::Ignore;
    let map = |i: Ignore| match i {
        Ignore::None => SubmoduleIgnore::None,
        Ignore::Untracked => SubmoduleIgnore::Untracked,
        Ignore::Dirty => SubmoduleIgnore::Dirty,
        Ignore::All => SubmoduleIgnore::All,
    };
    if let Some(value) = repo.config_snapshot().string("diff.ignoreSubmodules")
        && let Ok(i) = gix::config::tree::Diff::IGNORE_SUBMODULES.try_into_ignore(value)
    {
        return map(i);
    }
    let Ok(Some(mut submodules)) = repo.submodules() else {
        return SubmoduleIgnore::None;
    };
    submodules
        .find(|sm| sm.path().is_ok_and(|p| p.as_bytes() == rela_path))
        .and_then(|sm| sm.ignore().ok().flatten())
        .map_or(SubmoduleIgnore::None, map)
}

/// Whether the checked-out submodule at `rela_path` (gitlink `expected`) shows up as ` M`.
fn submodule_is_modified(
    repo: &gix::Repository,
    rela_path: &[u8],
    expected: gix::ObjectId,
    open: &dyn Fn(&std::path::Path) -> Option<gix::Repository>,
) -> bool {
    let ignore = submodule_ignore(repo, rela_path);
    if ignore == SubmoduleIgnore::All {
        return false;
    }
    let Some(workdir) = repo.workdir() else {
        return false;
    };
    let Ok(rel) = gix::path::from_byte_slice(rela_path) else {
        return false;
    };
    let sm_dir = workdir.join(rel);
    if !sm_dir.join(".git").exists() {
        return false; // not checked out: git shows nothing
    }
    let Some(sub) = open(&sm_dir) else {
        return false;
    };
    let new_commits = sub
        .head_id()
        .map(|id| id.detach() != expected)
        .unwrap_or(false);
    if new_commits || ignore == SubmoduleIgnore::Dirty {
        return new_commits;
    }
    let Some(entries) = collect(sub, open) else {
        return false;
    };
    entries.has_tracked_changes() || (ignore == SubmoduleIgnore::None && entries.has_untracked())
}

/// Rename tracking as `git status` configures it: `status.renames` falling back to
/// `diff.renames` (default on, `copies`/`copy` enables copy detection), and
/// `status.renameLimit` falling back to `diff.renameLimit`.
///
/// gix's own `AsConfigured` skips `diff.renameLimit` unless `diff.renames` is also set.
pub fn status_rewrites(repo: &gix::Repository) -> Option<gix::diff::Rewrites> {
    let config = repo.config_snapshot();
    // None: unset. Some(None): disabled. Some(Some(copies)): enabled.
    let tracking = |key: &str| -> Option<Option<bool>> {
        let value = config.string(key)?;
        let value = value.to_str_lossy().trim().to_ascii_lowercase();
        if value == "copies" || value == "copy" {
            return Some(Some(true));
        }
        match config.boolean(key) {
            Some(true) => Some(Some(false)),
            Some(false) => Some(None),
            None => None,
        }
    };
    let copies = tracking("status.renames")
        .or_else(|| tracking("diff.renames"))
        .unwrap_or(Some(false))?;
    let default = gix::diff::Rewrites::default();
    let limit = config
        .integer("status.renameLimit")
        .or_else(|| config.integer("diff.renameLimit"))
        .map(|l| l.max(0) as usize)
        .unwrap_or(default.limit);
    Some(gix::diff::Rewrites {
        copies: copies.then(gix::diff::rewrites::Copies::default),
        limit,
        ..default
    })
}

#[derive(Clone)]
struct Candidate {
    id: gix::ObjectId,
    kind: gix::diff::rewrites::tracker::ChangeKind,
    mode: gix::object::tree::EntryMode,
}

impl gix::diff::rewrites::tracker::Change for Candidate {
    fn id(&self) -> &gix::oid {
        &self.id
    }
    fn relation(&self) -> Option<gix::diff::tree::visit::Relation> {
        None
    }
    fn kind(&self) -> gix::diff::rewrites::tracker::ChangeKind {
        self.kind
    }
    fn entry_mode(&self) -> gix::object::tree::EntryMode {
        self.mode
    }
    fn id_and_entry_mode(&self) -> (&gix::oid, gix::object::tree::EntryMode) {
        (&self.id, self.mode)
    }
}

/// The blob git would store for the worktree file at `path`, without following symlinks.
///
/// A symlink's blob is its target path, like `git add`, so a link to `/dev/zero` costs nothing.
/// FIFOs, sockets, devices and directories are skipped: reading one could block forever or
/// never reach EOF.
fn worktree_blob(path: &std::path::Path) -> Option<(Vec<u8>, gix::object::tree::EntryKind)> {
    use gix::object::tree::EntryKind;
    use std::os::unix::ffi::OsStringExt;
    let md = std::fs::symlink_metadata(path).ok()?;
    if md.file_type().is_symlink() {
        let target = std::fs::read_link(path).ok()?;
        Some((target.into_os_string().into_vec(), EntryKind::Link))
    } else if md.file_type().is_file() {
        Some((std::fs::read(path).ok()?, EntryKind::Blob))
    } else {
        None
    }
}

/// git pairs an intent-to-add entry (` A`) with a file deleted from the worktree (` D`) and
/// prints a single ` R` line, which git-radar doesn't count. Do the same, with gix's rewrite
/// tracker limited to those two sets. ITA entries store the empty blob, so destinations are
/// hashed and read from the worktree.
fn pair_intent_to_add_with_deleted(
    repo: &gix::Repository,
    entries: &mut Entries,
    intent_to_add: Vec<BString>,
    deleted: Vec<(BString, gix::ObjectId, gix::index::entry::Mode)>,
    rewrites: gix::diff::Rewrites,
) {
    use gix::diff::rewrites::tracker::ChangeKind;
    let deleted: Vec<_> = deleted
        .into_iter()
        .filter(|(path, _, _)| entries.xy.get(path).is_some_and(|xy| xy.0 == b' '))
        .collect();
    if intent_to_add.is_empty() || deleted.is_empty() {
        return;
    }
    let Some(workdir) = repo.workdir().map(ToOwned::to_owned) else {
        return;
    };
    let mut tracker = gix::diff::rewrites::Tracker::new(gix::diff::Rewrites {
        copies: None,
        ..rewrites
    });
    for (path, id, mode) in &deleted {
        let Some(mode) = mode.to_tree_entry_mode() else {
            continue;
        };
        tracker.try_push_change(
            Candidate {
                id: *id,
                kind: ChangeKind::Deletion,
                mode,
            },
            path.as_ref(),
        );
    }
    for path in &intent_to_add {
        let Ok(rel) = gix::path::from_byte_slice(path) else {
            continue;
        };
        let Some((data, kind)) = worktree_blob(&workdir.join(rel)) else {
            continue;
        };
        let Ok(id) = gix::objs::compute_hash(repo.object_hash(), gix::object::Kind::Blob, &data)
        else {
            continue;
        };
        tracker.try_push_change(
            Candidate {
                id,
                kind: ChangeKind::Addition,
                mode: kind.into(),
            },
            path.as_ref(),
        );
    }
    let Ok(mut cache) = repo.diff_resource_cache(
        gix::diff::blob::pipeline::Mode::ToGit,
        gix::diff::blob::pipeline::WorktreeRoots {
            old_root: None,
            new_root: Some(workdir),
        },
    ) else {
        return;
    };
    let mut pairs = Vec::new();
    let res = tracker.emit(
        |dest, src| {
            if let Some(src) = src {
                pairs.push((src.location.to_owned(), dest.location.to_owned()));
            }
            std::ops::ControlFlow::Continue(())
        },
        &mut cache,
        &repo.objects,
        |_| Ok::<_, std::convert::Infallible>(()),
    );
    if res.is_err() {
        return;
    }
    for (src, dst) in pairs {
        entries.xy.remove(&src);
        entries.set_both(dst, b' ', b'R');
    }
}

/// Compute the porcelain `XY` pairs for `repo`. `open` opens submodules the same way the
/// superproject was opened. Returns `None` if status could not be computed.
pub fn collect(
    mut repo: gix::Repository,
    open: &dyn Fn(&std::path::Path) -> Option<gix::Repository>,
) -> Option<Entries> {
    repo.workdir()?;
    strip_filter_drivers(&mut repo);

    let rewrites = status_rewrites(&repo);
    let iter = repo
        .status(gix::progress::Discard)
        .ok()?
        // Submodules are checked below with our own open options. gix would reopen them with
        // the full config, including filter drivers.
        .index_worktree_submodules(None)
        // `git status` only pairs renames between HEAD and the index...
        .index_worktree_rewrites(None)
        // ...using `status.renames`/`status.renameLimit` with `diff.*` fallbacks.
        .tree_index_track_renames(match rewrites {
            Some(r) => gix::status::tree_index::TrackRenames::Given(r),
            None => gix::status::tree_index::TrackRenames::Disabled,
        })
        .into_iter(Vec::new())
        .ok()?;

    let mut entries = Entries::default();
    let mut conflicts = Vec::new();
    let mut copy_sources = Vec::new();
    let mut intent_to_add = Vec::new();
    let mut worktree_deleted = Vec::new();
    for item in iter {
        let item = item.ok()?;
        match item {
            gix::status::Item::TreeIndex(change) => {
                if let gix::diff::index::Change::Rewrite {
                    source_location,
                    copy: true,
                    ..
                } = &change
                {
                    // gix marks a modified copy source as consumed and never reports it, but
                    // git still lists the source as modified (`M`).
                    copy_sources.push(source_location.as_ref().to_owned());
                }
                let (path, x) = tree_index_letter(&change);
                entries.set_x(path, x);
            }
            gix::status::Item::IndexWorktree(wt) => match wt {
                WtItem::Modification {
                    rela_path,
                    status,
                    entry,
                    ..
                } => match &status {
                    EntryStatus::Conflict { summary, .. } => conflicts.push((rela_path, *summary)),
                    other => {
                        if let Some(y) = worktree_letter(other) {
                            match y {
                                b'A' => intent_to_add.push(rela_path.clone()),
                                b'D' => {
                                    worktree_deleted.push((rela_path.clone(), entry.id, entry.mode))
                                }
                                _ => {}
                            }
                            entries.set_y(rela_path, y);
                        }
                    }
                },
                WtItem::DirectoryContents { entry, .. } => {
                    if entry.status == gix::dir::entry::Status::Untracked {
                        entries.set_both(entry.rela_path, b'?', b'?');
                    }
                }
                WtItem::Rewrite { .. } => {} // rewrites are disabled above
            },
        }
    }
    for path in copy_sources {
        let xy = entries.xy.entry(path).or_insert((b' ', b' '));
        if xy.0 == b' ' {
            xy.0 = b'M';
        }
    }
    // Unmerged paths show only their conflict code.
    for (path, summary) in conflicts {
        let (x, y) = conflict_letters(summary);
        entries.set_both(path, x, y);
    }
    if let Some(rewrites) = rewrites {
        pair_intent_to_add_with_deleted(
            &repo,
            &mut entries,
            intent_to_add,
            worktree_deleted,
            rewrites,
        );
    }

    // Checked-out submodules whose content or HEAD moved show as ` M`.
    if let Ok(index) = repo.index_or_empty() {
        for entry in index.entries() {
            if entry.mode != gix::index::entry::Mode::COMMIT
                || entry.stage() != gix::index::entry::Stage::Unconflicted
            {
                continue;
            }
            let path = entry.path(&index);
            if submodule_is_modified(&repo, path, entry.id, open) {
                entries.set_y(path.to_owned(), b'M');
            }
        }
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::index::entry::Mode;

    fn count(x: u8, y: u8) -> Counts {
        let mut c = Counts::default();
        c.add(x, y);
        c
    }

    #[test]
    fn staged_rows() {
        for y in *b"MCDR " {
            assert_eq!(count(b'A', y).staged_added, 1, "A{}", y as char);
        }
        for y in *b"AMCR " {
            assert_eq!(count(b'D', y).staged_deleted, 1, "D{}", y as char);
        }
        for y in *b"ACDRM " {
            assert_eq!(count(b'M', y).staged_modified, 1, "M{}", y as char);
        }
        for y in *b"AMCD " {
            assert_eq!(count(b'R', y).staged_renamed, 1, "R{}", y as char);
        }
        for y in *b"AMDR " {
            assert_eq!(count(b'C', y).staged_copied, 1, "C{}", y as char);
        }
    }

    #[test]
    fn unstaged_rows() {
        for x in *b"AMCR " {
            assert_eq!(count(x, b'D').unstaged_deleted, 1, "{}D", x as char);
        }
        for x in *b"ACDRM " {
            assert_eq!(count(x, b'M').unstaged_modified, 1, "{}M", x as char);
        }
    }

    #[test]
    fn conflict_rows() {
        assert_eq!(count(b'A', b'U').conflict_us, 1);
        assert_eq!(count(b'D', b'U').conflict_us, 1);
        assert_eq!(count(b'U', b'A').conflict_them, 1);
        assert_eq!(count(b'U', b'D').conflict_them, 1);
        for (x, y) in [(b'U', b'U'), (b'A', b'A'), (b'D', b'D')] {
            let c = count(x, y);
            assert_eq!(c.conflict_both, 1);
            // AA and DD must not also count as staged or unstaged.
            assert_eq!(
                Counts {
                    conflict_both: 0,
                    ..c
                },
                Counts::default()
            );
        }
        assert_eq!(count(b'?', b'?').untracked, 1);
    }

    #[test]
    fn pairs_that_count_nothing() {
        for (x, y) in [
            (b'T', b' '),
            (b' ', b'T'),
            (b'M', b'T'),
            (b'A', b'T'),
            (b'T', b'M'),
            (b' ', b'A'),
            (b' ', b'R'),
            (b'R', b'R'),
        ] {
            assert_eq!(count(x, y), Counts::default(), "{}{}", x as char, y as char);
        }
    }

    #[test]
    fn mode_kinds() {
        assert_eq!(mode_kind(Mode::FILE), mode_kind(Mode::FILE_EXECUTABLE));
        assert_ne!(mode_kind(Mode::FILE), mode_kind(Mode::SYMLINK));
        assert_ne!(mode_kind(Mode::FILE), mode_kind(Mode::COMMIT));
        assert_ne!(mode_kind(Mode::SYMLINK), mode_kind(Mode::COMMIT));
    }

    #[test]
    fn worktree_letters() {
        let s: EntryStatus = EntryStatus::Change(Change::Type {
            worktree_mode: Mode::SYMLINK,
        });
        assert_eq!(worktree_letter(&s), Some(b'T'));
        let s: EntryStatus = EntryStatus::Change(Change::Modification {
            executable_bit_changed: true,
            content_change: None,
            set_entry_stat_size_zero: false,
        });
        assert_eq!(worktree_letter(&s), Some(b'M'));
        let s: EntryStatus = EntryStatus::Change(Change::Removed);
        assert_eq!(worktree_letter(&s), Some(b'D'));
        let s: EntryStatus = EntryStatus::IntentToAdd;
        assert_eq!(worktree_letter(&s), Some(b'A'));
    }
}
