//! Golden tests: `radar --bash` must be byte-identical to the installed git-radar 0.6
//! (`/opt/homebrew/bin/git-radar`, override with `GIT_RADAR_BIN`) on the same fixture repo.
//!
//! Oracle format, checked with `git-radar --bash | od -c`: git-radar writes **raw bytes**
//! (`\001 ESC [ ... m \002`, UTF-8 symbols, no trailing newline). `render_prompt` runs
//! `printf '%b'` and every segment is printed with `printf "$segment"`, so all escapes are
//! interpreted before output. The oracle is therefore its raw stdout, not the post-`echo -e` text.
//!
//! Every command (fixture `git`, the oracle, radar) runs with `env_clear()` and an explicit env.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DEFAULT_PATH: &str = "/opt/homebrew/bin:/usr/bin:/bin";

fn oracle_bin() -> Option<PathBuf> {
    let p = std::env::var_os("GIT_RADAR_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/git-radar"));
    p.exists().then_some(p)
}

/// Skip the test when git-radar isn't installed, or fail it when `RADAR_REQUIRE_ORACLE` is set
/// (CI sets it, so a broken oracle install can't turn the suite into silent skips).
macro_rules! require_oracle {
    () => {
        if oracle_bin().is_none() {
            assert!(
                std::env::var_os("RADAR_REQUIRE_ORACLE").is_none(),
                "git-radar not found and RADAR_REQUIRE_ORACLE is set"
            );
            eprintln!("skipping: git-radar not found (set GIT_RADAR_BIN)");
            return;
        }
    };
}

#[derive(Clone)]
struct Env {
    vars: BTreeMap<String, String>,
}

impl Env {
    fn new(home: &Path) -> Self {
        let mut vars = BTreeMap::new();
        for (k, v) in [
            ("PATH", DEFAULT_PATH),
            ("LANG", "C"),
            ("LC_ALL", "C"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Radar Test"),
            ("GIT_AUTHOR_EMAIL", "radar@example.com"),
            ("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z"),
            ("GIT_COMMITTER_NAME", "Radar Test"),
            ("GIT_COMMITTER_EMAIL", "radar@example.com"),
            ("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z"),
        ] {
            vars.insert(k.to_owned(), v.to_owned());
        }
        vars.insert("HOME".to_owned(), home.display().to_string());
        Env { vars }
    }
    fn set(mut self, k: &str, v: impl AsRef<str>) -> Self {
        self.vars.insert(k.to_owned(), v.as_ref().to_owned());
        self
    }
    #[cfg_attr(not(debug_assertions), allow(dead_code))] // only debug-only tests use it
    fn unset(mut self, k: &str) -> Self {
        self.vars.remove(k);
        self
    }
    fn apply<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env_clear().envs(&self.vars)
    }
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: Env,
}

fn show(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace(['\x01', '\x02'], "")
        .replace("\x1b[0m", "{0}")
        .replace("\x1b[", "{")
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let env = Env::new(&root.join("home"));
        Fx {
            _tmp: tmp,
            root,
            env,
        }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn run_in(&self, env: &Env, cwd: &Path, program: &str, args: &[&str]) -> Output {
        let mut cmd = Command::new(program);
        env.apply(&mut cmd).current_dir(cwd).args(args);
        cmd.output()
            .unwrap_or_else(|e| panic!("{program} {args:?}: {e}"))
    }

    /// Run git and require success.
    fn git(&self, cwd: &str, args: &[&str]) -> String {
        let out = self.run_in(&self.env, &self.p(cwd), "git", args);
        assert!(
            out.status.success(),
            "git {args:?} in {cwd} failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Run git, tolerating failure (e.g. a merge that conflicts).
    fn git_any(&self, cwd: &str, args: &[&str]) {
        self.run_in(&self.env, &self.p(cwd), "git", args);
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.p(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn commit(&self, repo: &str, file: &str, content: &str, msg: &str) {
        self.write(&format!("{repo}/{file}"), content);
        self.git(repo, &["add", "--", file]);
        self.git(repo, &["commit", "-q", "-m", msg]);
    }

    /// `remote.git` (bare) plus a work repo `w` on `branch` with one pushed commit and upstream set.
    fn with_remote(&self, branch: &str) {
        self.git(".", &["init", "-q", "--bare", "-b", branch, "remote.git"]);
        self.git(".", &["init", "-q", "-b", branch, "w"]);
        self.commit("w", "README", "hello\n", "init");
        self.git("w", &["remote", "add", "origin", "../remote.git"]);
        self.git("w", &["push", "-q", "-u", "origin", branch]);
    }

    fn radar_env(&self, env: &Env, cwd: &str) -> Output {
        self.run_in(env, &self.p(cwd), env!("CARGO_BIN_EXE_radar"), &["--bash"])
    }
    fn radar(&self, cwd: &str) -> Vec<u8> {
        self.radar_with(&self.env, cwd)
    }
    fn radar_with(&self, env: &Env, cwd: &str) -> Vec<u8> {
        let out = self.radar_env(env, cwd);
        assert!(out.status.success());
        assert!(
            out.stderr.is_empty(),
            "radar wrote to stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    fn oracle(&self, cwd: &str) -> Vec<u8> {
        self.oracle_with(&self.env, cwd)
    }
    fn oracle_with(&self, env: &Env, cwd: &str) -> Vec<u8> {
        let bin = oracle_bin().unwrap();
        self.run_in(env, &self.p(cwd), bin.to_str().unwrap(), &["--bash"])
            .stdout
    }

    #[track_caller]
    fn same(&self, cwd: &str) -> Vec<u8> {
        self.same_with(&self.env, cwd)
    }
    #[track_caller]
    fn same_with(&self, env: &Env, cwd: &str) -> Vec<u8> {
        let (o, r) = (self.oracle_with(env, cwd), self.radar_with(env, cwd));
        assert_eq!(
            show(&r),
            show(&o),
            "radar (left) vs git-radar (right) in {cwd}"
        );
        assert_eq!(r, o, "byte mismatch in {cwd}");
        o
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------------------------
// basics

#[test]
fn not_a_repo_prints_nothing() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.p("plain")).unwrap();
    assert!(fx.radar("plain").is_empty());
}

#[test]
fn clean_on_master() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.same("w");
}

#[test]
fn no_upstream() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "local-only"]);
    fx.same("w");
}

#[test]
fn unborn_repo() {
    require_oracle!();
    let fx = Fx::new();
    fx.git(".", &["init", "-q", "-b", "main", "w"]);
    fx.same("w");
    fx.write("w/staged", "s\n");
    fx.git("w", &["add", "staged"]);
    fx.write("w/untracked", "u\n");
    fx.same("w");
}

#[test]
fn cwd_in_subdir() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "dir/sub/file", "a\n", "dir");
    fx.write("w/dir/sub/file", "b\n");
    fx.write("w/top-untracked", "u\n");
    fx.same("w/dir/sub");
}

#[test]
fn cwd_inside_dot_git() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.write("w/x", "dirty\n");
    fx.write("w/README", "changed\n");
    fx.git("w", &["stash", "-q"]);
    fx.write("w/README", "changed again\n");
    fx.same("w/.git");
    fx.same("w/.git/refs");
}

#[test]
fn cwd_in_dot_github_shows_counts() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", ".github/workflows/ci.yml", "on: push\n", "ci");
    fx.write("w/.github/workflows/ci.yml", "on: pull_request\n");
    fx.write("w/.github/workflows/new.yml", "x\n");
    let out = fx.same("w/.github/workflows");
    assert!(contains(&out, b"M"), "counts must show in .github");
}

#[test]
fn linked_worktree() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["worktree", "add", "-q", "-b", "wt-branch", "../wt"]);
    fx.write("wt/README", "wt change\n");
    fx.write("wt/new", "n\n");
    fx.same("wt");
    fx.same("w");
}

fn submodule_fixture(fx: &Fx) {
    fx.with_remote("master");
    fx.git(".", &["init", "-q", "-b", "master", "sub"]);
    fx.commit("sub", "s", "s1\n", "s1");
    fx.git(
        "w",
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            "../sub",
            "sub",
        ],
    );
    fx.git("w", &["commit", "-q", "-m", "add sub"]);
}

#[test]
fn submodule_clean_and_variants() {
    require_oracle!();
    let fx = Fx::new();
    submodule_fixture(&fx);
    fx.same("w");
    // untracked content
    fx.write("w/sub/untracked", "u\n");
    fx.same("w");
    // modified content
    fx.write("w/sub/s", "modified\n");
    fx.same("w");
    // new commits (and clean content)
    fx.git("w/sub", &["checkout", "-q", "--", "s"]);
    std::fs::remove_file(fx.p("w/sub/untracked")).unwrap();
    fx.commit("w/sub", "s", "s2\n", "s2");
    fx.same("w");
    // staged gitlink change
    fx.git("w", &["add", "sub"]);
    fx.same("w");
}

#[test]
fn submodule_ignore_config() {
    require_oracle!();
    let fx = Fx::new();
    submodule_fixture(&fx);
    fx.write("w/sub/untracked", "u\n");
    fx.git("w", &["config", "submodule.sub.ignore", "untracked"]);
    fx.same("w");
    fx.write("w/sub/s", "modified\n");
    fx.git("w", &["config", "submodule.sub.ignore", "dirty"]);
    fx.same("w");
    fx.git("w", &["config", "submodule.sub.ignore", "all"]);
    fx.commit("w/sub", "s", "s2\n", "s2");
    fx.same("w");
    fx.git("w", &["config", "--unset", "submodule.sub.ignore"]);
    fx.git("w", &["config", "diff.ignoreSubmodules", "all"]);
    fx.same("w");
}

/// `w` with submodule `mid`, which has its own submodule `inner`, both checked out.
fn nested_submodule_fixture(fx: &Fx) {
    let file = ["-c", "protocol.file.allow=always"];
    fx.with_remote("master");
    for repo in ["inner", "mid"] {
        fx.git(".", &["init", "-q", "-b", "master", repo]);
        fx.commit(repo, "f", &format!("{repo}1\n"), repo);
    }
    fx.git(
        "mid",
        &[&file[..], &["submodule", "add", "-q", "../inner", "inner"]].concat(),
    );
    fx.git("mid", &["commit", "-q", "-m", "add inner"]);
    fx.git(
        "w",
        &[&file[..], &["submodule", "add", "-q", "../mid", "mid"]].concat(),
    );
    fx.git("w", &["commit", "-q", "-m", "add mid"]);
    fx.git(
        "w",
        &[
            &file[..],
            &["submodule", "update", "-q", "--init", "--recursive"],
        ]
        .concat(),
    );
    assert!(fx.p("w/mid/inner/f").is_file());
}

#[test]
fn nested_submodules() {
    require_oracle!();
    let fx = Fx::new();
    nested_submodule_fixture(&fx);
    let clean = fx.same("w");
    // modified content two levels down
    fx.write("w/mid/inner/f", "modified\n");
    assert_ne!(fx.same("w"), clean);
    // untracked content two levels down
    fx.git("w/mid/inner", &["checkout", "-q", "--", "f"]);
    fx.write("w/mid/inner/untracked", "u\n");
    fx.same("w");
    // new commits two levels down
    std::fs::remove_file(fx.p("w/mid/inner/untracked")).unwrap();
    fx.commit("w/mid/inner", "f", "inner2\n", "inner2");
    fx.same("w");
}

/// Past the depth limit a submodule's contents aren't scanned, but its HEAD is still compared.
#[cfg(debug_assertions)]
#[test]
fn submodule_depth_limit() {
    require_oracle!();
    let fx = Fx::new();
    nested_submodule_fixture(&fx);
    let depth = |n: usize| {
        fx.env
            .clone()
            .set("RADAR_TEST_SUBMODULE_DEPTH", n.to_string())
    };
    let clean = fx.same("w");
    fx.write("w/mid/inner/f", "modified\n");
    let dirty = fx.same("w");
    assert_ne!(dirty, clean);
    assert_eq!(show(&fx.radar_with(&depth(2), "w")), show(&dirty));
    // `inner` is at depth 2: not scanned, so `mid` looks clean.
    assert_eq!(show(&fx.radar_with(&depth(1), "w")), show(&clean));
    assert_eq!(show(&fx.radar_with(&depth(0), "w")), show(&clean));
    // `mid`'s HEAD moving is still seen without scanning it.
    fx.git("w/mid/inner", &["checkout", "-q", "--", "f"]);
    fx.commit("w/mid", "g", "g\n", "mid2");
    let moved = fx.same("w");
    assert_ne!(moved, clean);
    assert_eq!(show(&fx.radar_with(&depth(0), "w")), show(&moved));
}

// ---------------------------------------------------------------------------------------------
// rename configuration (status.* falls back to diff.*)

fn lines(n: usize, tag: &str) -> String {
    (0..n)
        .map(|i| format!("{tag} line number {i} with some padding text\n"))
        .collect()
}

fn global_config(fx: &Fx, body: &str) -> Env {
    let path = fx.p("global.gitconfig");
    std::fs::write(&path, body).unwrap();
    fx.env
        .clone()
        .set("GIT_CONFIG_GLOBAL", path.display().to_string())
}

#[test]
fn diff_renames_false_fallback() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "old.txt", &lines(20, "r"), "old");
    fx.git("w", &["mv", "old.txt", "new.txt"]);
    let default = fx.same("w");
    let env = global_config(&fx, "[diff]\n\trenames = false\n");
    let no_renames = fx.same_with(&env, "w");
    assert_ne!(
        default, no_renames,
        "fixture must exercise the rename setting"
    );
}

#[test]
fn diff_renames_copies_fallback() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "a.txt", &lines(24, "c"), "a");
    // First stage a small edit to `a` (copy sources must be modified), then copy it.
    let mut edited = lines(24, "c");
    edited.push_str("one more line\n");
    fx.write("w/a.txt", &edited);
    fx.git("w", &["add", "a.txt"]);
    fx.write("w/b.txt", &edited);
    fx.git("w", &["add", "b.txt"]);
    let default = fx.same("w");
    let env = global_config(&fx, "[diff]\n\trenames = copies\n");
    let copies = fx.same_with(&env, "w");
    assert_ne!(
        default, copies,
        "fixture must exercise copy detection (expected C vs A)"
    );
}

#[test]
fn diff_rename_limit_fallback() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "a.txt", &lines(24, "alpha"), "a");
    fx.commit("w", "c.txt", &lines(24, "gamma"), "c");
    // Inexact renames with different basenames, so the limit applies.
    fx.git("w", &["rm", "-q", "a.txt", "c.txt"]);
    let mut b = lines(24, "alpha");
    b.push_str("edited\n");
    let mut d = lines(24, "gamma");
    d.push_str("edited\n");
    fx.write("w/b.txt", &b);
    fx.write("w/d.txt", &d);
    fx.git("w", &["add", "b.txt", "d.txt"]);
    let default = fx.same("w");
    let env = global_config(&fx, "[diff]\n\trenameLimit = 1\n");
    let limited = fx.same_with(&env, "w");
    assert_ne!(
        default, limited,
        "fixture must exercise the rename limit (expected 2R vs 2D 2A)"
    );
}

// ---------------------------------------------------------------------------------------------
// changes

#[test]
fn staged_and_unstaged_mix() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    for f in ["mod", "del", "ren", "mm", "unmod", "undel"] {
        fx.commit("w", f, &lines(10, f), f);
    }
    fx.write("w/added", "new\n");
    fx.git("w", &["add", "added"]);
    fx.write("w/am", "am\n");
    fx.git("w", &["add", "am"]);
    fx.write("w/am", "am changed\n");
    fx.write("w/mod", "modified\n");
    fx.git("w", &["add", "mod"]);
    fx.git("w", &["rm", "-q", "del"]);
    fx.git("w", &["mv", "ren", "renamed"]);
    fx.write("w/mm", "staged\n");
    fx.git("w", &["add", "mm"]);
    fx.write("w/mm", "and unstaged\n");
    fx.write("w/unmod", "unstaged\n");
    std::fs::remove_file(fx.p("w/undel")).unwrap();
    fx.same("w");
}

#[test]
fn three_stashes() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    for i in 0..3 {
        fx.write("w/README", &format!("stash {i}\n"));
        fx.git("w", &["stash", "-q"]);
    }
    fx.same("w");
}

#[test]
fn all_seven_conflict_kinds_via_index_stages() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    let blob = |content: &str| {
        fx.write("w/.blob", content);
        fx.git("w", &["hash-object", "-w", ".blob"])
    };
    let (b1, b2, b3) = (blob("base\n"), blob("ours\n"), blob("theirs\n"));
    std::fs::remove_file(fx.p("w/.blob")).unwrap();
    // stages: 1=base 2=ours 3=theirs
    let kinds: [(&str, &[(u8, &str)]); 7] = [
        ("dd", &[(1, &b1)]),
        ("au", &[(2, &b2)]),
        ("ud", &[(1, &b1), (2, &b2)]),
        ("ua", &[(3, &b3)]),
        ("du", &[(1, &b1), (3, &b3)]),
        ("aa", &[(2, &b2), (3, &b3)]),
        ("uu", &[(1, &b1), (2, &b2), (3, &b3)]),
    ];
    let mut info = String::new();
    for (path, stages) in kinds {
        for (stage, id) in stages {
            info.push_str(&format!("100644 {id} {stage}\t{path}\n"));
        }
        if stages.iter().any(|(s, _)| *s != 1) {
            fx.write(&format!("w/{path}"), "worktree\n");
        }
    }
    let mut cmd = Command::new("git");
    fx.env
        .apply(&mut cmd)
        .current_dir(fx.p("w"))
        .args(["update-index", "--index-info"]);
    cmd.stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(info.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    fx.same("w");
}

#[test]
fn real_merge_conflicts() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "both", "base\n", "both");
    fx.commit("w", "deleted-by-us", "base\n", "dbu");
    fx.commit("w", "deleted-by-them", "base\n", "dbt");
    fx.git("w", &["checkout", "-q", "-b", "other"]);
    fx.commit("w", "both", "other\n", "o1");
    fx.commit("w", "added", "other\n", "o2");
    fx.git("w", &["rm", "-q", "deleted-by-them"]);
    fx.git("w", &["commit", "-q", "-m", "o3"]);
    fx.commit("w", "deleted-by-us", "other change\n", "o4");
    fx.git("w", &["checkout", "-q", "master"]);
    fx.commit("w", "both", "master\n", "m1");
    fx.commit("w", "added", "master\n", "m2");
    fx.commit("w", "deleted-by-them", "master change\n", "m3");
    fx.git("w", &["rm", "-q", "deleted-by-us"]);
    fx.git("w", &["commit", "-q", "-m", "m4"]);
    fx.git_any("w", &["merge", "-q", "other"]);
    fx.same("w");
}

#[test]
fn mid_rebase_detached_with_conflicts() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "topic"]);
    fx.commit("w", "README", "topic\n", "t1");
    fx.commit("w", "f2", "topic\n", "t2");
    fx.git("w", &["push", "-q", "-u", "origin", "topic"]);
    fx.git("w", &["checkout", "-q", "master"]);
    fx.commit("w", "README", "master\n", "m1");
    fx.commit("w", "f2", "master\n", "m2");
    fx.git("w", &["push", "-q", "origin", "master"]);
    fx.git("w", &["checkout", "-q", "topic"]);
    fx.git_any("w", &["rebase", "master"]);
    fx.same("w");
}

#[test]
fn untracked_collapsing_and_invisible_dirs() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", ".gitignore", "*.log\n", "ignore");
    fx.write("w/u1", "u\n");
    fx.write("w/u2", "u\n");
    fx.write("w/newdir/a", "a\n");
    fx.write("w/newdir/deeper/b", "b\n");
    std::fs::create_dir_all(fx.p("w/emptydir/nested")).unwrap();
    fx.write("w/ignored-only/x.log", "x\n");
    fx.write("w/ignored-only/deep/y.log", "y\n");
    fx.same("w");
}

#[test]
fn show_untracked_files_no() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.write("w/u1", "u\n");
    fx.git("w", &["config", "status.showUntrackedFiles", "no"]);
    fx.same("w");
}

#[test]
fn worktree_delete_plus_copy_is_not_a_rename() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "orig.txt", &lines(20, "w"), "orig");
    std::fs::remove_file(fx.p("w/orig.txt")).unwrap();
    fx.write("w/moved.txt", &lines(20, "w"));
    fx.same("w");
}

#[test]
fn intent_to_add() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.write("w/ita.txt", "intent\n");
    fx.git("w", &["add", "-N", "ita.txt"]);
    fx.same("w");
}

#[test]
fn intent_to_add_plus_deleted_same_content() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "gone.txt", &lines(20, "ita"), "gone");
    std::fs::remove_file(fx.p("w/gone.txt")).unwrap();
    fx.write("w/here.txt", &lines(20, "ita"));
    fx.git("w", &["add", "-N", "here.txt"]);
    fx.same("w");
}

#[test]
fn intent_to_add_plus_deleted_similar_content() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "gone.txt", &lines(20, "ita"), "gone");
    std::fs::remove_file(fx.p("w/gone.txt")).unwrap();
    let mut similar: String = lines(14, "ita");
    similar.push_str(&lines(6, "different"));
    fx.write("w/here.txt", &similar);
    fx.git("w", &["add", "-N", "here.txt"]);
    fx.same("w");
}

/// Over the big-file threshold an intent-to-add file pairs only with identical content. git
/// also pairs similar content, but that needs both versions in memory.
#[cfg(debug_assertions)]
#[test]
fn intent_to_add_big_files_pair_only_when_identical() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    let big = fx.env.clone().set("RADAR_TEST_BIG_FILE_THRESHOLD", "64");
    fx.commit("w", "gone.txt", &lines(20, "ita"), "gone");
    std::fs::remove_file(fx.p("w/gone.txt")).unwrap();
    // identical: paired, like git
    fx.write("w/here.txt", &lines(20, "ita"));
    fx.git("w", &["add", "-N", "here.txt"]);
    let paired = fx.same("w");
    assert_eq!(show(&fx.radar_with(&big, "w")), show(&paired));
    // similar: git pairs them, radar shows the addition and the deletion
    let mut similar = lines(14, "ita");
    similar.push_str(&lines(6, "different"));
    fx.write("w/here.txt", &similar);
    let git = fx.same("w");
    let no_renames = global_config(&fx, "[status]\n\trenames = false\n");
    let unpaired = fx.oracle_with(&no_renames, "w");
    assert_ne!(show(&unpaired), show(&git));
    assert_eq!(show(&fx.radar_with(&big, "w")), show(&unpaired));
}

/// Intent-to-add files bigger than the heap cap must not blank the prompt.
#[test]
#[ignore = "writes 2.2 GiB of (sparse) files"]
fn intent_to_add_bigger_than_memory_cap() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    let size = (1 << 30) + (64 << 20);
    let sparse = |rel: &str, tail: &[u8]| {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::File::create(fx.p(rel)).unwrap();
        f.set_len(size).unwrap();
        f.seek(SeekFrom::End(-(tail.len() as i64))).unwrap();
        f.write_all(tail).unwrap();
    };
    sparse("w/gone.bin", b"end\n");
    fx.git("w", &["add", "gone.bin"]);
    fx.git("w", &["commit", "-q", "-m", "big"]);
    std::fs::remove_file(fx.p("w/gone.bin")).unwrap();
    // identical: paired
    sparse("w/here.bin", b"end\n");
    fx.git("w", &["add", "-N", "here.bin"]);
    let g = radar_guarded(&fx, &fx.env, "w", HANG_LIMIT, 1024);
    assert!(g.out.status.success() && g.out.stderr.is_empty());
    assert_eq!(show(&g.out.stdout), show(&fx.oracle("w")));
    // similar: not paired, so the deletion shows (git-radar doesn't count ` A`), but still a
    // prompt
    sparse("w/here.bin", b"END\n");
    let g = radar_guarded(&fx, &fx.env, "w", HANG_LIMIT, 1024);
    assert!(g.out.status.success() && g.out.stderr.is_empty());
    let out = show(&g.out.stdout);
    assert!(out.contains("git:(") && out.contains("1{1;31mD"), "{out}");
}

/// Mark `w/<name>` intent-to-add, then replace the worktree file with whatever `make` creates.
fn intent_to_add_then_replace(fx: &Fx, name: &str, make: impl FnOnce(&Path)) {
    fx.write(&format!("w/{name}"), "placeholder\n");
    fx.git("w", &["add", "-N", name]);
    let path = fx.p(&format!("w/{name}"));
    std::fs::remove_file(&path).unwrap();
    make(&path);
}

fn mkfifo(path: &Path) {
    let status = Command::new("/usr/bin/mkfifo").arg(path).status().unwrap();
    assert!(status.success(), "mkfifo {}", path.display());
}

struct Guarded {
    out: Output,
    elapsed: std::time::Duration,
}

/// Run radar in `cwd` under an external watchdog: kill it past `limit` or past `rss_mb` of
/// resident memory. A hang or runaway allocation then fails the test, instead of blocking the
/// suite or exhausting the machine.
#[track_caller]
fn radar_guarded(
    fx: &Fx,
    env: &Env,
    cwd: &str,
    limit: std::time::Duration,
    rss_mb: u64,
) -> Guarded {
    use std::process::Stdio;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_radar"));
    env.apply(&mut cmd)
        .current_dir(fx.p(cwd))
        .arg("--bash")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let pid = child.id().to_string();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        let rss_mb_now = Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map_or(0, |kb| kb / 1024);
        if rss_mb_now > rss_mb || start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "radar in {cwd} killed after {:?} at {rss_mb_now} MB resident",
                start.elapsed()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let elapsed = start.elapsed();
    Guarded {
        out: child.wait_with_output().unwrap(),
        elapsed,
    }
}

#[track_caller]
fn assert_radar_finishes(fx: &Fx, cwd: &str, limit: std::time::Duration) {
    radar_guarded(fx, &fx.env, cwd, limit, 1024);
}

const HANG_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// An intent-to-add symlink is hashed by its link text, like git, never by what it points to.
/// The target here is a FIFO, so following the link would block forever. A FIFO is used rather
/// than `/dev/zero`: a regression would then hang, and be killed, instead of eating all memory.
#[test]
fn intent_to_add_symlink_is_not_followed() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "gone.txt", &lines(20, "ita"), "gone");
    std::fs::remove_file(fx.p("w/gone.txt")).unwrap();
    mkfifo(&fx.p("fifo"));
    intent_to_add_then_replace(&fx, "new", |p| {
        std::os::unix::fs::symlink("../fifo", p).unwrap()
    });
    assert_radar_finishes(&fx, "w", HANG_LIMIT);
    fx.same("w");
}

/// git pairs a deleted symlink with an intent-to-add symlink to the same target, but never a
/// symlink with a regular file whose content equals the link text.
#[test]
fn intent_to_add_symlink_pairs_only_with_symlinks() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    std::os::unix::fs::symlink("README", fx.p("w/old-link")).unwrap();
    fx.commit("w", "same-text", "README", "link and file");
    fx.git("w", &["add", "old-link"]);
    fx.git("w", &["commit", "-q", "-m", "link"]);

    // symlink -> symlink, same target: paired, so nothing is shown.
    std::fs::remove_file(fx.p("w/old-link")).unwrap();
    intent_to_add_then_replace(&fx, "new-link", |p| {
        std::os::unix::fs::symlink("README", p).unwrap()
    });
    fx.same("w");

    // regular file "README" -> symlink to README: not paired.
    fx.git("w", &["reset", "-q", "--hard"]);
    std::fs::remove_file(fx.p("w/same-text")).unwrap();
    intent_to_add_then_replace(&fx, "link-to-readme", |p| {
        std::os::unix::fs::symlink("README", p).unwrap()
    });
    fx.same("w");
}

/// An intent-to-add path that is now a FIFO is skipped instead of read. git never pairs it
/// with a deleted file of this size either.
#[test]
fn intent_to_add_fifo_is_not_read() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "gone.txt", &lines(20, "ita"), "gone");
    std::fs::remove_file(fx.p("w/gone.txt")).unwrap();
    intent_to_add_then_replace(&fx, "new", mkfifo);
    assert_radar_finishes(&fx, "w", HANG_LIMIT);
    fx.same("w");
}

#[test]
fn typechanges() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "staged-tc", "file\n", "f1");
    fx.commit("w", "unstaged-tc", "file\n", "f2");
    fx.commit("w", "exec-bit", "#!/bin/sh\n", "f3");
    std::fs::remove_file(fx.p("w/staged-tc")).unwrap();
    std::os::unix::fs::symlink("README", fx.p("w/staged-tc")).unwrap();
    fx.git("w", &["add", "staged-tc"]);
    std::fs::remove_file(fx.p("w/unstaged-tc")).unwrap();
    std::os::unix::fs::symlink("README", fx.p("w/unstaged-tc")).unwrap();
    fx.same("w");
    // An exec-bit flip is a modification, not a typechange.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(fx.p("w/exec-bit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.same("w");
    fx.git("w", &["add", "exec-bit"]);
    fx.same("w");
}

// ---------------------------------------------------------------------------------------------
// upstream

#[test]
fn local_ahead_behind_diverged() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "a", "1\n", "a1");
    fx.commit("w", "a", "2\n", "a2");
    fx.same("w"); // ahead 2
    fx.git("w", &["push", "-q"]);
    fx.git("w", &["reset", "-q", "--hard", "HEAD~2"]);
    fx.same("w"); // behind 2
    fx.commit("w", "b", "1\n", "b1");
    fx.same("w"); // diverged
}

#[test]
fn upstream_vs_master_ahead_behind_diverged() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "feature"]);
    fx.commit("w", "f", "1\n", "f1");
    fx.git("w", &["push", "-q", "-u", "origin", "feature"]);
    fx.same("w"); // upstream ahead of master
    fx.git("w", &["checkout", "-q", "master"]);
    fx.commit("w", "m", "1\n", "m1");
    fx.commit("w", "m", "2\n", "m2");
    fx.git("w", &["push", "-q", "origin", "master"]);
    fx.git("w", &["checkout", "-q", "feature"]);
    fx.same("w"); // diverged
    fx.git("w", &["checkout", "-q", "-b", "behind", "origin/master~2"]);
    fx.git("w", &["push", "-q", "-u", "origin", "behind"]);
    fx.same("w"); // upstream behind master
}

#[test]
fn upstream_with_different_name() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "feat"]);
    fx.commit("w", "f", "1\n", "f1");
    fx.git("w", &["push", "-q", "-u", "origin", "feat:feature-x"]);
    fx.commit("w", "f", "2\n", "f2");
    fx.same("w");
}

#[test]
fn two_branches_tracking_same_remote_branch() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "shared"]);
    fx.commit("w", "s", "1\n", "s1");
    fx.git("w", &["push", "-q", "-u", "origin", "shared"]);
    fx.git(
        "w",
        &["checkout", "-q", "-b", "second", "--track", "origin/shared"],
    );
    fx.commit("w", "s", "2\n", "s2");
    fx.same("w");
    fx.git("w", &["checkout", "-q", "shared"]);
    fx.same("w");
}

#[test]
fn upstream_remote_branch_deleted() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "gone"]);
    fx.commit("w", "g", "1\n", "g1");
    fx.git("w", &["push", "-q", "-u", "origin", "gone"]);
    fx.git("w", &["push", "-q", "origin", "--delete", "gone"]);
    fx.same("w");
}

#[test]
fn upstream_is_local_branch() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git(
        "w",
        &["checkout", "-q", "-b", "local-up", "--track", "master"],
    );
    fx.commit("w", "l", "1\n", "l1");
    assert_eq!(fx.git("w", &["config", "branch.local-up.remote"]), ".");
    fx.same("w");
}

#[test]
fn detached_head() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.commit("w", "x", "1\n", "x1");
    fx.git("w", &["checkout", "-q", "--detach", "HEAD~1"]);
    fx.same("w");
}

#[test]
fn branch_names_are_not_mangled() {
    // Documented divergence: git-radar runs the branch through `sed` and `printf "$fmt"`, so
    // `&` becomes the matched pattern and `%d` a number. radar prints the name verbatim.
    let fx = Fx::new();
    fx.with_remote("master");
    for name in ["a&b", "pct%dx"] {
        fx.git("w", &["checkout", "-q", "-b", name]);
        let out = fx.radar("w");
        assert!(
            contains(&out, format!("\x02{name}\x01").as_bytes()),
            "{}",
            show(&out)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// graph stress

#[test]
fn criss_cross_merges() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    // Two branches that merge each other twice → two merge bases.
    fx.git("w", &["checkout", "-q", "-b", "x"]);
    fx.commit("w", "x", "1\n", "x1");
    fx.git("w", &["checkout", "-q", "-b", "y", "master"]);
    fx.commit("w", "y", "1\n", "y1");
    fx.git("w", &["checkout", "-q", "x"]);
    fx.git("w", &["merge", "-q", "--no-ff", "-m", "x<-y", "y~0"]);
    fx.git("w", &["checkout", "-q", "y"]);
    fx.git("w", &["merge", "-q", "--no-ff", "-m", "y<-x1", "x~1"]);
    fx.commit("w", "y", "2\n", "y2");
    fx.git("w", &["checkout", "-q", "x"]);
    fx.commit("w", "x", "2\n", "x2");
    // HEAD x vs upstream origin/x at y; upstream vs origin/master criss-crossed too.
    fx.git("w", &["push", "-q", "origin", "y:x"]);
    fx.git("w", &["branch", "-q", "--set-upstream-to=origin/x"]);
    fx.git("w", &["push", "-q", "origin", "x:master", "--force"]);
    fx.same("w");
}

#[test]
fn long_divergence_with_merges() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "-b", "long"]);
    fx.git("w", &["push", "-q", "-u", "origin", "long"]);
    for i in 0..150 {
        fx.commit("w", "l", &format!("{i}\n"), &format!("l{i}"));
    }
    fx.git("w", &["checkout", "-q", "-b", "upside", "origin/long"]);
    for i in 0..120 {
        fx.commit("w", "u", &format!("{i}\n"), &format!("u{i}"));
        if i % 30 == 29 {
            fx.git(
                "w",
                &[
                    "merge",
                    "-q",
                    "--no-ff",
                    "-m",
                    "merge long",
                    &format!("long~{}", 150 - i),
                ],
            );
        }
    }
    fx.git("w", &["push", "-q", "origin", "upside:long"]);
    fx.git("w", &["checkout", "-q", "master"]);
    for i in 0..40 {
        fx.commit("w", "m", &format!("{i}\n"), &format!("m{i}"));
    }
    fx.git("w", &["push", "-q", "origin", "master"]);
    fx.git("w", &["checkout", "-q", "long"]);
    fx.same("w");
    fx.git("w", &["commit-graph", "write", "--reachable"]);
    fx.same("w");
}

#[test]
fn shallow_clones() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    for i in 0..10 {
        fx.commit("w", "h", &format!("{i}\n"), &format!("h{i}"));
    }
    fx.git("w", &["push", "-q"]);
    let url = format!("file://{}", fx.p("remote.git").display());
    fx.git(".", &["clone", "-q", "--depth", "1", &url, "s1"]);
    fx.same("s1");
    fx.git(".", &["clone", "-q", "--depth", "5", &url, "s5"]);
    fx.commit("w", "h", "upstream\n", "upstream gains");
    fx.git("w", &["push", "-q"]);
    fx.git("s5", &["fetch", "-q"]);
    fx.same("s5"); // behind
    fx.commit("s5", "local", "1\n", "local");
    fx.same("s5"); // diverged
    fx.git("s5", &["reset", "-q", "--hard", "origin/master"]);
    fx.commit("s5", "local", "1\n", "local");
    fx.same("s5"); // ahead
}

// ---------------------------------------------------------------------------------------------
// bare

#[test]
fn bare_repositories() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.same("remote.git");
    std::fs::create_dir_all(fx.p("outside")).unwrap();
    let env = fx
        .env
        .clone()
        .set("GIT_DIR", fx.p("remote.git").display().to_string());
    fx.same_with(&env, "outside");
}

// ---------------------------------------------------------------------------------------------
// config

#[test]
fn core_abbrev_from_global_config() {
    require_oracle!();
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "--detach"]);
    let env = global_config(&fx, "[core]\n\tabbrev = 10\n");
    let out = fx.same_with(&env, "w");
    assert!(contains(&out, b"detached@"));
}

#[cfg(debug_assertions)]
fn detached_sha_len(out: &[u8]) -> usize {
    let s = String::from_utf8_lossy(out);
    let rest = s.split("detached@").nth(1).unwrap();
    rest.chars().take_while(|c| c.is_ascii_hexdigit()).count()
}

#[cfg(debug_assertions)]
#[test]
fn installation_config_is_honored_and_nosystem_disables_it() {
    let fx = Fx::new();
    fx.with_remote("master");
    fx.git("w", &["checkout", "-q", "--detach"]);
    fx.write("install.gitconfig", "[core]\n\tabbrev = 12\n");
    let env = fx.env.clone().unset("GIT_CONFIG_NOSYSTEM").set(
        "RADAR_TEST_INSTALL_CONFIG",
        fx.p("install.gitconfig").display().to_string(),
    );
    assert_eq!(detached_sha_len(&fx.radar_with(&env, "w")), 12);
    let env = env.set("GIT_CONFIG_NOSYSTEM", "1");
    assert_eq!(detached_sha_len(&fx.radar_with(&env, "w")), 7);
}

// ---------------------------------------------------------------------------------------------
// main branch detection (the reason this project exists)

/// Same history in `w` with default branch `default`; HEAD on `feature` tracking origin/feature,
/// which is ahead of, behind, and diverged from the default branch over the steps.
fn main_fixture(fx: &Fx, default: &str) {
    fx.with_remote(default);
    fx.git("w", &["checkout", "-q", "-b", "feature"]);
    fx.commit("w", "f", "1\n", "f1");
    fx.git("w", &["push", "-q", "-u", "origin", "feature"]);
}

fn main_fixture_diverge(fx: &Fx, default: &str) {
    main_fixture_diverge_on(fx, "origin", default);
}

fn main_fixture_diverge_on(fx: &Fx, remote: &str, default: &str) {
    fx.git("w", &["checkout", "-q", default]);
    fx.commit("w", "m", "1\n", "m1");
    fx.git("w", &["push", "-q", remote, default]);
    fx.git("w", &["checkout", "-q", "feature"]);
}

#[test]
fn main_branch_is_detected() {
    require_oracle!();
    let (a, b) = (Fx::new(), Fx::new());
    main_fixture(&a, "master");
    main_fixture(&b, "main");
    assert_eq!(b.radar("w"), a.oracle("w"), "ahead of main");
    assert!(contains(&b.radar("w"), "𝘮".as_bytes()));
    main_fixture_diverge(&a, "master");
    main_fixture_diverge(&b, "main");
    assert_eq!(
        show(&b.radar("w")),
        show(&a.oracle("w")),
        "diverged from main"
    );
}

#[test]
fn origin_head_points_to_develop() {
    require_oracle!();
    let (a, b) = (Fx::new(), Fx::new());
    main_fixture(&a, "master");
    main_fixture(&b, "develop");
    // A decoy `master` that must lose against origin/HEAD.
    b.git("w", &["push", "-q", "origin", "feature:master"]);
    b.git("w", &["remote", "set-head", "origin", "develop"]);
    main_fixture_diverge(&a, "master");
    main_fixture_diverge(&b, "develop");
    assert_eq!(show(&b.radar("w")), show(&a.oracle("w")));
}

#[test]
fn radar_main_branch_config_override() {
    require_oracle!();
    let (a, b) = (Fx::new(), Fx::new());
    main_fixture(&a, "master");
    main_fixture(&b, "trunk");
    b.git("w", &["config", "radar.mainBranch", "origin/trunk"]);
    main_fixture_diverge(&a, "master");
    main_fixture_diverge(&b, "trunk");
    assert_eq!(show(&b.radar("w")), show(&a.oracle("w")));
}

#[test]
fn tracked_remote_named_upstream() {
    require_oracle!();
    let (a, b) = (Fx::new(), Fx::new());
    main_fixture(&a, "master");
    main_fixture_diverge(&a, "master");
    b.git(".", &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    b.git(".", &["init", "-q", "-b", "main", "w"]);
    b.commit("w", "README", "hello\n", "init");
    b.git("w", &["remote", "add", "upstream", "../remote.git"]);
    b.git("w", &["push", "-q", "-u", "upstream", "main"]);
    b.git("w", &["checkout", "-q", "-b", "feature"]);
    b.commit("w", "f", "1\n", "f1");
    b.git("w", &["push", "-q", "-u", "upstream", "feature"]);
    b.git("w", &["remote", "set-head", "upstream", "main"]);
    main_fixture_diverge_on(&b, "upstream", "main");
    assert_eq!(show(&b.radar("w")), show(&a.oracle("w")));
}

#[test]
fn head_on_main_skips_master_segment() {
    require_oracle!();
    let (a, b) = (Fx::new(), Fx::new());
    a.with_remote("master");
    b.with_remote("main");
    a.commit("w", "x", "1\n", "x");
    b.commit("w", "x", "1\n", "x");
    let expected = String::from_utf8(a.oracle("w"))
        .unwrap()
        .replace("master", "main");
    assert_eq!(show(&b.radar("w")), show(expected.as_bytes()));
    assert!(!contains(&b.radar("w"), "𝘮".as_bytes()));
}

// ---------------------------------------------------------------------------------------------
// filter drivers are never executed

fn logger_script(fx: &Fx, transform: bool) -> (PathBuf, PathBuf) {
    let log = fx.p("filter.log");
    let script = fx.p("logger.sh");
    let body = if transform {
        format!(
            "#!/bin/sh\necho run >> '{}'\nprintf 'pointer v1\\n'\ncat\n",
            log.display()
        )
    } else {
        format!("#!/bin/sh\necho run >> '{}'\nexit 1\n", log.display())
    };
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

fn touch(path: &Path, stamp: &str) {
    assert!(
        Command::new("/usr/bin/touch")
            .args(["-m", "-t", stamp])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

fn log_len(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[test]
fn clean_filter_is_not_run() {
    let fx = Fx::new();
    fx.with_remote("master");
    let (script, log) = logger_script(&fx, true);
    fx.git(
        "w",
        &["config", "filter.logger.clean", script.to_str().unwrap()],
    );
    fx.commit("w", ".gitattributes", "*.bin filter=logger\n", "attrs");
    fx.commit("w", "data.bin", "payload\n", "data");
    // Sanity: a stat-dirty file makes git run the filter.
    touch(&fx.p("w/data.bin"), "203001010000");
    fx.git("w", &["status", "--porcelain"]);
    assert!(log_len(&log) > 0, "fixture must trigger the filter in git");
    touch(&fx.p("w/data.bin"), "203101010000");
    std::fs::write(&log, "").unwrap();
    let out = fx.radar("w");
    assert_eq!(log_len(&log), 0, "radar ran the clean filter");
    // Documented divergence: without the filter the raw file differs from the stored blob.
    assert!(contains(&out, b"1\x01\x1b[1;31m\x02M"), "{}", show(&out));
}

#[test]
fn process_filter_is_not_launched() {
    let fx = Fx::new();
    fx.with_remote("master");
    let (script, log) = logger_script(&fx, false);
    fx.commit("w", "data.bin", "payload\n", "data");
    fx.commit("w", ".gitattributes", "*.bin filter=logger\n", "attrs");
    fx.git(
        "w",
        &["config", "filter.logger.process", script.to_str().unwrap()],
    );
    touch(&fx.p("w/data.bin"), "203001010000");
    std::fs::write(&log, "").unwrap();
    fx.radar("w");
    assert_eq!(log_len(&log), 0, "radar launched the process filter");
}

#[test]
fn clean_filter_in_submodule_is_not_run() {
    let fx = Fx::new();
    let (script, log) = logger_script(&fx, true);
    let env = global_config(
        &fx,
        &format!("[filter \"logger\"]\n\tclean = {}\n", script.display()),
    );
    let fx = Fx { env, ..fx };
    fx.with_remote("master");
    fx.git(".", &["init", "-q", "-b", "master", "sub"]);
    fx.commit("sub", ".gitattributes", "*.bin filter=logger\n", "attrs");
    fx.commit("sub", "data.bin", "payload\n", "data");
    fx.git(
        "w",
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            "../sub",
            "sub",
        ],
    );
    fx.git("w", &["commit", "-q", "-m", "add sub"]);
    touch(&fx.p("w/sub/data.bin"), "203001010000");
    fx.git("w/sub", &["status", "--porcelain"]);
    assert!(log_len(&log) > 0, "fixture must trigger the filter in git");
    touch(&fx.p("w/sub/data.bin"), "203101010000");
    std::fs::write(&log, "").unwrap();
    fx.radar("w");
    assert_eq!(
        log_len(&log),
        0,
        "radar ran the clean filter inside the submodule"
    );
}

// ---------------------------------------------------------------------------------------------
// process hygiene

#[test]
fn no_git_subprocess() {
    let fx = Fx::new();
    submodule_fixture(&fx);
    fx.git("w", &["checkout", "-q", "-b", "feature"]);
    fx.commit("w", "f", "1\n", "f1");
    fx.git("w", &["push", "-q", "-u", "origin", "feature"]);
    fx.commit("w", "f", "2\n", "f2");
    fx.write("w/README", "dirty\n");
    fx.git("w", &["stash", "-q"]);
    fx.write("w/sub/s", "dirty\n");
    fx.write("w/untracked", "u\n");
    let normal = fx.radar("w");

    let shim_dir = fx.p("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log = fx.p("shim.log");
    std::fs::write(
        shim_dir.join("git"),
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", log.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(shim_dir.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = fx
        .env
        .clone()
        .set("PATH", format!("{}:/usr/bin:/bin", shim_dir.display()));
    let shimmed = fx.radar_with(&env, "w");
    assert_eq!(
        log_len(&log),
        0,
        "radar spawned git: {:?}",
        std::fs::read_to_string(&log)
    );
    assert_eq!(show(&shimmed), show(&normal));
}

#[cfg(debug_assertions)]
#[test]
fn panic_prints_nothing_and_exits_zero() {
    let fx = Fx::new();
    fx.with_remote("master");
    let env = fx.env.clone().set("RADAR_TEST_PANIC", "after_branch");
    let out = fx.radar_env(&env, "w");
    assert!(out.status.success(), "exit status {:?}", out.status);
    assert!(out.stdout.is_empty(), "stdout: {}", show(&out.stdout));
    assert!(
        out.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------------------------
// process guard: hostile files in `.git` must not hang the prompt or exhaust memory

/// Places in `.git` that gix reads to EOF. Pointed at `/dev/zero` they allocate without bound
/// (`index` is the exception: it's only a FIFO hazard); pointed at a FIFO they block forever.
#[cfg(debug_assertions)]
const HOSTILE_PATHS: &[&str] = &[
    "HEAD",
    "packed-refs",
    "stash reflog",
    "info/exclude",
    "core.excludesFile",
    "core.attributesFile",
    "branch ref",
];

/// A fresh `w` repo whose `what` points at `target`.
fn hostile_repo(what: &str, target: &Path) -> Fx {
    let fx = Fx::new();
    fx.with_remote("master");
    let git_path = |rel: &str| fx.p(&format!("w/.git/{rel}"));
    let replace = |rel: &str| {
        let path = git_path(rel);
        let _ = std::fs::remove_file(&path);
        std::os::unix::fs::symlink(target, &path).unwrap();
    };
    let target_str = target.to_str().unwrap();
    match what {
        "HEAD" => replace("HEAD"),
        "packed-refs" => {
            fx.git("w", &["pack-refs", "--all"]);
            replace("packed-refs");
        }
        "stash reflog" => {
            fx.write("w/README", "changed\n");
            fx.git("w", &["stash", "-q"]);
            replace("logs/refs/stash");
        }
        "info/exclude" => replace("info/exclude"),
        "core.excludesFile" => {
            fx.git("w", &["config", "core.excludesFile", target_str]);
        }
        "core.attributesFile" => {
            fx.git("w", &["config", "core.attributesFile", target_str]);
        }
        "branch ref" => replace("refs/heads/master"),
        "index" => replace("index"),
        other => panic!("unknown hostile path {other}"),
    }
    fx
}

#[track_caller]
fn assert_silent_success(g: &Guarded, what: &str) {
    assert!(g.out.status.success(), "{what}: exit {:?}", g.out.status);
    assert!(
        g.out.stdout.is_empty(),
        "{what}: stdout {}",
        show(&g.out.stdout)
    );
    assert!(
        g.out.stderr.is_empty(),
        "{what}: stderr {}",
        String::from_utf8_lossy(&g.out.stderr)
    );
}

/// With a 64 MiB cap, each `/dev/zero` read stops at the cap: silent exit 0, long before the
/// 5 s watchdog. The external ceiling (512 MB) only matters if the cap is broken.
#[test]
#[cfg(debug_assertions)]
fn memory_cap_stops_reads_from_dev_zero() {
    for what in HOSTILE_PATHS {
        let fx = hostile_repo(what, Path::new("/dev/zero"));
        let env = fx
            .env
            .clone()
            .set("RADAR_TEST_MEMORY_LIMIT", (64u64 << 20).to_string());
        let g = radar_guarded(&fx, &env, "w", std::time::Duration::from_secs(20), 512);
        assert_silent_success(&g, what);
        assert!(
            g.elapsed < std::time::Duration::from_secs(4),
            "{what}: took {:?}, so the watchdog fired, not the memory cap",
            g.elapsed
        );
    }
}

/// With a 300 ms watchdog, each FIFO read is abandoned: silent exit 0.
#[test]
#[cfg(debug_assertions)]
fn watchdog_stops_fifo_reads() {
    for what in HOSTILE_PATHS.iter().chain(&["index"]) {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("fifo");
        mkfifo(&fifo);
        let fx = hostile_repo(what, &fifo);
        let env = fx.env.clone().set("RADAR_TEST_TIME_LIMIT_MS", "300");
        let g = radar_guarded(&fx, &env, "w", std::time::Duration::from_secs(10), 512);
        assert_silent_success(&g, what);
        assert!(
            g.elapsed < std::time::Duration::from_secs(4),
            "{what}: took {:?}",
            g.elapsed
        );
    }
}

/// The real 5 s limit, in both debug and release builds.
#[test]
fn default_time_limit_is_five_seconds() {
    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("fifo");
    mkfifo(&fifo);
    let fx = hostile_repo("HEAD", &fifo);
    let g = radar_guarded(&fx, &fx.env, "w", std::time::Duration::from_secs(15), 512);
    assert_silent_success(&g, "HEAD -> fifo");
    let secs = g.elapsed.as_secs_f64();
    assert!((4.9..9.0).contains(&secs), "gave up after {secs:.2}s");
}

/// The real 1 GiB cap. Ignored by default: it allocates 1 GiB, and the suite runs tests in
/// parallel.
#[test]
#[ignore = "allocates 1 GiB"]
fn default_memory_cap_is_one_gib() {
    let fx = hostile_repo("HEAD", Path::new("/dev/zero"));
    let g = radar_guarded(&fx, &fx.env, "w", std::time::Duration::from_secs(20), 1400);
    assert_silent_success(&g, "HEAD -> /dev/zero");
    assert!(
        g.elapsed < std::time::Duration::from_secs(4),
        "took {:?}",
        g.elapsed
    );
}

// ---------------------------------------------------------------------------------------------
// abbreviation length in big repositories

#[test]
#[ignore = "slow: builds a repository with >40k objects"]
fn abbrev_length_grows_with_object_count() {
    let fx = Fx::new();
    fx.git(".", &["init", "-q", "-b", "main", "big"]);
    let mut stream = String::new();
    let n = 42_000;
    for i in 1..=n {
        let data = format!("blob {i}\n");
        stream.push_str(&format!("blob\nmark :{i}\ndata {}\n{data}\n", data.len()));
    }
    stream.push_str("commit refs/heads/main\ncommitter T <t@t> 1577836800 +0000\ndata 3\nbig\n");
    for i in 1..=n {
        stream.push_str(&format!("M 100644 :{i} f/{}/{i}\n", i % 100));
    }
    let mut cmd = Command::new("git");
    fx.env
        .apply(&mut cmd)
        .current_dir(fx.p("big"))
        .args(["fast-import", "--quiet"]);
    cmd.stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    fx.git(
        "big",
        &["update-ref", "--no-deref", "HEAD", "refs/heads/main"],
    );
    let short = fx.git("big", &["rev-parse", "--short", "HEAD"]);
    assert!(short.len() > 7, "fixture too small: {short}");
    let out = String::from_utf8(fx.radar("big")).unwrap();
    assert!(
        out.contains(&format!("detached@{short}")),
        "{} vs {short}",
        show(out.as_bytes())
    );
}
