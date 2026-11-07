//! Opening the repository and gathering everything the prompt shows.

use std::path::Path;

use gix::bstr::ByteSlice;

use crate::installcfg::InstallCfg;
use crate::render::{State, Upstream};
use crate::{graph, mainbranch, status};

/// Open options that never run the `git` binary, with explicit installation/system config
/// paths, and that refuse repositories without full trust (git's "dubious ownership").
fn configure(mut opts: gix::open::Options, cfg: &InstallCfg) -> gix::open::Options {
    opts.permissions.config.git_binary = cfg.installation.is_some();
    opts.permissions.attributes.git_binary = false;
    if let Some(system) = &cfg.system {
        opts = opts.system_config_path(system);
    }
    if let Some(installation) = &cfg.installation {
        opts = opts.git_installation_config_path(installation);
    }
    opts.bail_if_untrusted(true)
}

fn install_cfg() -> InstallCfg {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("RADAR_TEST_INSTALL_CONFIG") {
        let mut cfg = InstallCfg {
            system: None,
            installation: Some(path.into()),
        };
        if crate::installcfg::env_truthy(std::env::var_os("GIT_CONFIG_NOSYSTEM")) {
            cfg.installation = None;
        }
        return cfg;
    }
    crate::installcfg::lookup(
        std::env::var_os("PATH"),
        &|k| std::env::var_os(k),
        Path::new("/"),
    )
}

pub struct Opener {
    cfg: InstallCfg,
}

impl Opener {
    pub fn new() -> Self {
        Opener { cfg: install_cfg() }
    }

    fn mapping(&self) -> gix::sec::trust::Mapping<gix::open::Options> {
        use gix::sec::trust::DefaultForLevel;
        gix::sec::trust::Mapping {
            full: configure(
                gix::open::Options::default_for_level(gix::sec::Trust::Full),
                &self.cfg,
            ),
            reduced: configure(
                gix::open::Options::default_for_level(gix::sec::Trust::Reduced),
                &self.cfg,
            ),
        }
    }

    /// Discover the repository containing `cwd`, honoring `GIT_DIR` and friends.
    pub fn discover(&self, cwd: &Path) -> Option<gix::Repository> {
        let repo = gix::ThreadSafeRepository::discover_with_environment_overrides_opts(
            cwd,
            Default::default(),
            self.mapping(),
        )
        .ok()?
        .to_thread_local();
        (repo.git_dir_trust() == gix::sec::Trust::Full).then_some(repo)
    }

    /// Open a submodule worktree at `dir` with the same options.
    pub fn open_submodule(&self, dir: &Path) -> Option<gix::Repository> {
        let repo = gix::ThreadSafeRepository::discover_opts(
            dir,
            gix::discover::upwards::Options {
                // A submodule dir always has its own `.git`; never walk up into the superproject.
                ceiling_dirs: dir.parent().map(|p| vec![p.to_owned()]).unwrap_or_default(),
                ..Default::default()
            },
            self.mapping(),
        )
        .ok()?
        .to_thread_local();
        let found = repo.workdir().and_then(|w| w.canonicalize().ok());
        if found != dir.canonicalize().ok() {
            return None;
        }
        (repo.git_dir_trust() == gix::sec::Trust::Full).then_some(repo)
    }
}

/// `git config --get <section>.<sub>.<key>`: the last value wins.
fn config_string(repo: &gix::Repository, section: &str, sub: &[u8], key: &str) -> Option<String> {
    let config = repo.config_snapshot();
    let value = config
        .plumbing()
        .string_by(section, Some(sub.as_bstr()), key)?;
    Some(value.to_str_lossy().into_owned())
}

/// Whether `cwd` is inside the git dir, where `git status` and `git stash list` refuse to run.
/// Compared component-wise, so `<repo>/.github` doesn't match `<repo>/.git`.
fn cwd_in_git_dir(repo: &gix::Repository, cwd: &Path) -> bool {
    let Ok(cwd) = cwd.canonicalize() else {
        return false;
    };
    [repo.git_dir(), repo.common_dir()]
        .iter()
        .filter_map(|d| d.canonicalize().ok())
        .any(|d| cwd.starts_with(d))
}

fn stash_count(repo: &gix::Repository) -> usize {
    let Ok(Some(stash)) = repo.try_find_reference("refs/stash") else {
        return 0;
    };
    let mut log = stash.log_iter();
    match log.all() {
        Ok(Some(iter)) => iter.filter(|line| line.is_ok()).count(),
        _ => 0,
    }
}

/// Everything shown in the prompt for the repository containing `cwd`. `None` if not in a repo.
pub fn gather(cwd: &Path) -> Option<State> {
    let opener = Opener::new();
    let repo = opener.discover(cwd)?;

    // Branch, like `git symbolic-ref --short HEAD || detached@$(git rev-parse --short HEAD)`.
    let head_name = repo.head_name().ok().flatten();
    let branch_short: Option<Vec<u8>> = head_name.as_ref().map(|n| n.shorten().to_vec());
    let branch = match &branch_short {
        Some(name) => name.clone(),
        None => {
            let short = repo
                .head_id()
                .ok()
                .and_then(|id| id.shorten().ok())
                .map(|p| p.to_string())
                .unwrap_or_default();
            format!("detached@{short}").into_bytes()
        }
    };

    // Upstream, like git-radar's `remote_branch_name`: `branch.<b>.remote` + `/` +
    // `branch.<b>.merge` without `refs/heads/`, resolved as a revision (DWIM), not via refspecs.
    let mut branch_remote = None;
    let upstream = branch_short.as_ref().and_then(|b| {
        let remote = config_string(&repo, "branch", b, "remote").filter(|r| !r.is_empty())?;
        let merge = config_string(&repo, "branch", b, "merge").filter(|m| !m.is_empty())?;
        let merge = merge
            .strip_prefix("refs/heads/")
            .unwrap_or(&merge)
            .to_owned();
        branch_remote = Some(remote.clone());
        Some(format!("{remote}/{merge}"))
    });
    let upstream = upstream.map(|spec| {
        let mut up = Upstream::default();
        let upstream_id = repo
            .rev_parse_single(spec.as_str())
            .ok()
            .and_then(|id| id.object().ok()?.peel_to_commit().ok())
            .map(|c| c.id);
        let head_id = repo.head_id().ok().map(|id| id.detach());
        if let (Some(up_id), Some(head)) = (upstream_id, head_id)
            && let Some((behind, ahead)) = graph::left_right(&repo, up_id, head)
        {
            up.behind = behind;
            up.ahead = ahead;
        }
        if let Some(up_id) = upstream_id
            && let Some(main) = mainbranch::resolve(&repo, branch_remote.as_deref())
            && let Some((behind, ahead)) = graph::left_right(&repo, main, up_id)
        {
            up.main_behind = behind;
            up.main_ahead = ahead;
        }
        up
    });

    let has_worktree_context = repo.workdir().is_some() && !cwd_in_git_dir(&repo, cwd);
    let (stash, changes) = if has_worktree_context {
        let open_sub = |dir: &Path| opener.open_submodule(dir);
        let changes = status::collect(repo.clone(), &open_sub).map(|e| e.counts());
        (stash_count(&repo), changes)
    } else {
        (0, None)
    };

    Some(State {
        branch,
        upstream,
        stash,
        changes,
    })
}

#[cfg(test)]
mod tests {
    use gix::config::Source;

    /// Untrusted repositories are refused (`bail_if_untrusted`), and only gix's trust logic can
    /// promote them via `safe.directory`. That logic must read `safe.directory` from system and
    /// global config only, so a repository can't trust itself through its own `.git/config`.
    /// A real ownership mismatch needs root, so this checks the filter gix applies.
    #[test]
    fn safe_directory_is_never_read_from_repository_config() {
        let filter = gix::config::tree::Safe::directory_filter;
        let meta = |source: Source| gix::config::file::Metadata::from(source);
        assert!(!filter(&meta(Source::Local)));
        assert!(!filter(&meta(Source::Worktree)));
        assert!(filter(&meta(Source::User)));
        assert!(filter(&meta(Source::Git)));
        assert!(filter(&meta(Source::System)));
    }

    #[test]
    fn untrusted_repositories_are_refused() {
        use gix::sec::trust::DefaultForLevel;
        let cfg = crate::installcfg::InstallCfg::default();
        let mut opts = super::configure(
            gix::open::Options::default_for_level(gix::sec::Trust::Reduced),
            &cfg,
        );
        // Keep the developer's own `safe.directory` settings out of this test.
        opts.permissions.config.system = false;
        opts.permissions.config.git = false;
        opts.permissions.config.user = false;
        // `bail_if_untrusted` has no getter. Opening a reduced-trust repo with these options must fail.
        let tmp = tempfile::tempdir().unwrap();
        let repo = gix::init(tmp.path()).unwrap();
        let res = opts.with(gix::sec::Trust::Reduced).open(repo.git_dir());
        assert!(res.is_err(), "reduced trust must not open");
    }
}
