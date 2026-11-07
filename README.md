# radar

A drop-in replacement for [git-radar](https://github.com/michaeldfallen/git-radar)'s
`--bash` output, written in Rust on top of [gitoxide](https://github.com/GitoxideLabs/gitoxide).

* **It recognizes `main`.** git-radar hard-codes `origin/master` for the `𝘮` segment.
* **It doesn't spawn processes.** git-radar runs ~8 `git` calls and ~15 `grep | wc` pipelines
  per prompt. radar runs none (see [Scope of "no subprocesses"](#scope-of-no-subprocesses)).
* **The output is byte-identical** to git-radar 0.6's default bash format. A golden test suite
  compares it against the real git-radar.

```
$ radar --bash
 git:(𝘮 ← 1 feat 2↑) 1≡ 1M 2A
```

## Usage

```bash
cargo install --path . --locked     # installs ~/.cargo/bin/radar
```

In `~/.bashrc`, or in a function used by `PS1`:

```bash
if command -v radar >/dev/null; then
  r=$(radar --bash 2>/dev/null); [ -n "$r" ] && printf '%s' "$r"
fi
```

The output is raw bytes. Escape sequences are already wrapped in readline's `\001`/`\002`
markers, so no `echo -e` is needed. It starts with a space, like git-radar's. Outside a
repository radar prints nothing. It always exits 0 and never writes to stderr.

## Which branch is "main"

The `𝘮` segment compares your upstream with the first of:

1. `git config radar.mainBranch`, any revision, e.g. `origin/develop`
2. for the current branch's remote, then `origin`:
   1. `refs/remotes/<remote>/HEAD`. Set by `git clone`, or `git remote set-head <remote> -a`.
   2. `refs/remotes/<remote>/main`
   3. `refs/remotes/<remote>/master`

If none exists, the segment is omitted.

## Differences from git-radar 0.6

Besides main-branch detection, these differences are intended:

* **Status codes are matched per path.** git-radar greps the whole `git status --porcelain`
  text, so a file *named* like a status code (e.g. `x M y`) inflates the counts. radar classifies
  each path by its XY code, using git-radar's exact pair table.
* **Branch names are printed verbatim.** git-radar passes branch names through `sed` and `printf`,
  so `a&b` renders as `a%{branch}b` and `pct%dx` as `pct0x`.
* **Filter drivers are never run** (git-lfs, `filter.<x>.clean`/`.process`). Running one is a
  subprocess. When a filtered file's stat info is stale (e.g. after `touch`), radar compares raw
  bytes with the stored blob and may report ` M` where git reports nothing. radar never writes
  the index, so this shows **on every prompt** until some git command, like `git status`,
  refreshes the index.

## Limitations

* **No fsmonitor or untracked cache.** gitoxide doesn't use `core.fsmonitor` or `core.untrackedCache`.
  In very large repos where those are enabled, `git status` can be much faster than radar, on every prompt.
  Without them, radar is close to `git status` (see the benchmarks).
* **Installation config is found by layout, not by asking git.** radar finds the first `git` on
  `PATH` with `stat` and maps known layouts to their config files:
  * Homebrew or `<prefix>/bin/git` → `<prefix>/etc/gitconfig`
  * Apple's `/usr/bin/git` → `/etc/gitconfig`, plus `<developer dir>/usr/share/git-core/gitconfig`

  A git in an unrecognized location has its installation config ignored. Apple git's
  installation-level `safe.directory` is also ignored, because gitoxide reads `safe.directory`
  only from system and global config.
* **Untrusted repositories print nothing**, like git's "dubious ownership" refusal. That covers
  repos owned by another user and not allowed by `safe.directory`.
* **bash only.** There's no `--zsh`/`--fish`, no `--fetch`, and no `GIT_RADAR_*` or `.gitradarrc` customization.

### Scope of "no subprocesses"

What's tested: radar never spawns `git`. A test replaces `git` on `PATH` with a logging shim.
Filter drivers are not run in the tested configurations: a `clean` filter, a `process` filter,
and a `clean` filter inside a submodule. Other helpers git might run (hooks, `core.fsmonitor`
hooks, textconv) aren't used by anything radar does, but no test asserts that.

## Development

```bash
cargo test                       # unit + golden tests (golden needs git-radar installed)
cargo test -- --ignored          # also the slow >40k-object abbreviation test
GIT_RADAR_BIN=/path/to/git-radar cargo test
scripts/bench.sh <repo-dir>      # hyperfine: git-radar vs radar vs `git status`
```

The golden tests build each fixture repo with the `git` CLI in a temp dir with a fully cleared
environment, then compare `radar --bash` to `git-radar --bash` byte for byte. They cover
conflicts of every kind, rebases, renames and copies, `diff.*` fallbacks, intent-to-add,
typechanges, submodules, worktrees, bare and shallow repos, criss-cross merges, and the
main-branch cases.

### Benchmarks

Measured on an Apple Silicon Mac with `scripts/bench.sh` (hyperfine `-N --warmup 5`):

| repo                           | git-radar | radar   | `git status --porcelain` |
|--------------------------------|-----------|---------|--------------------------|
| 2 files, upstream, stash       | 524 ms    | 13.8 ms | 28.3 ms                  |
| 60 000 files (synthetic)       | 833 ms    | 265 ms  | 281 ms                   |
