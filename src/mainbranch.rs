//! Default-branch resolution. This replaces git-radar's hardcoded `origin/master`.
//!
//! Order:
//! 1. `radar.mainBranch` (any revision, e.g. `origin/develop`)
//! 2. for the current branch's remote, then `origin`:
//!    `refs/remotes/<R>/HEAD`, `refs/remotes/<R>/main`, `refs/remotes/<R>/master`

use gix::ObjectId;

/// Peel `name` to a commit id.
fn commit_of(repo: &gix::Repository, spec: &str) -> Option<ObjectId> {
    let id = repo.rev_parse_single(spec).ok()?;
    let commit = id.object().ok()?.peel_to_commit().ok()?;
    Some(commit.id)
}

fn ref_commit(repo: &gix::Repository, name: &str) -> Option<ObjectId> {
    let mut reference = repo.find_reference(name).ok()?;
    let id = reference.peel_to_id().ok()?;
    let commit = id.object().ok()?.peel_to_commit().ok()?;
    Some(commit.id)
}

/// Commit at the tip of the main branch, if one can be determined.
pub fn resolve(repo: &gix::Repository, branch_remote: Option<&str>) -> Option<ObjectId> {
    let config = repo.config_snapshot();
    if let Some(spec) = config.string("radar.mainBranch") {
        let spec = spec.to_string();
        return commit_of(repo, spec.trim());
    }
    let mut remotes: Vec<&str> = Vec::new();
    if let Some(r) = branch_remote.filter(|r| !r.is_empty() && *r != ".") {
        remotes.push(r);
    }
    if !remotes.contains(&"origin") {
        remotes.push("origin");
    }
    for remote in remotes {
        for leaf in ["HEAD", "main", "master"] {
            if let Some(id) = ref_commit(repo, &format!("refs/remotes/{remote}/{leaf}")) {
                return Some(id);
            }
        }
    }
    None
}
