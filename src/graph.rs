//! Ahead/behind counting, matching `git rev-list --left-only/--right-only --count A...B`.

use gix::ObjectId;

/// Number of commits reachable from `tip` but not from `hidden`.
pub fn count_only(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId) -> Option<usize> {
    if tip == hidden {
        return Some(0);
    }
    let walk = repo.rev_walk([tip]).with_hidden([hidden]).all().ok()?;
    let mut n = 0;
    for info in walk {
        info.ok()?;
        n += 1;
    }
    Some(n)
}

/// `(left_only, right_only)` of `left...right`.
pub fn left_right(
    repo: &gix::Repository,
    left: ObjectId,
    right: ObjectId,
) -> Option<(usize, usize)> {
    Some((
        count_only(repo, left, right)?,
        count_only(repo, right, left)?,
    ))
}
