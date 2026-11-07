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

In `~/.bashrc`:

```bash
# Look radar up once, when this file is sourced, not on every prompt.
if [[ -x $HOME/.cargo/bin/radar ]]; then
    _radar_bin=$HOME/.cargo/bin/radar
else
    _radar_bin=$(command -v radar 2> /dev/null)
fi

radar_prompt() {
    [[ -n $_radar_bin ]] || return 0
    local out
    out=$("$_radar_bin" --bash 2> /dev/null)
    [[ -n $out ]] && printf '%s' "$out"
}

PS1='\w$(radar_prompt) \$ '
```

The output is raw bytes. Escape sequences are already wrapped in readline's `\001`/`\002`
markers, so no `echo -e` is needed. It starts with a space, like git-radar's. Outside a
repository radar prints nothing. It always exits 0 and never writes to stderr.

**Keep `$(…)` inside `PS1`, never radar's output.** Branch names may contain `$(…)` and
backticks (`git check-ref-format` allows them), and radar prints them verbatim, like git-radar.
With the setup above that's harmless: bash runs radar at each prompt and doesn't expand its
output again. It is **not** harmless if radar's output is pasted into `PS1` itself, as in
`PROMPT_COMMAND='PS1="\w$(radar --bash) \$ "'`. bash then expands the branch name, so cloning a
repository whose default branch is `$(cmd)` and `cd`-ing into it runs `cmd`. git-radar has
the same problem with that setup; git fixed the same bug in its own `git-prompt.sh` in 2014.

**Coming from git-radar?** The usual `echo -e $(git-radar --bash)` is unquoted, so it drops
the leading space. If your `PS1` already has a space before the segment, print
`"${out# }"` instead of `"$out"` to keep the prompt looking the same.

radar also prints nothing if it runs longer than **5 seconds** or allocates more than **1 GiB** of
heap. radar runs in every directory you `cd` into, and some files in `.git` are read to the end.
A planted symlink to `/dev/zero` (as `HEAD`, `packed-refs`, a reflog or an excludes file) would
otherwise use up all memory, and a FIFO would hang the prompt. A repository that's legitimately
slower than 5 s gets an empty prompt instead of a slow one.

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
* **Huge intent-to-add files pair with deleted files only when identical.** git pairs an
  intent-to-add file with a similar deleted file and shows neither. Over 512 MiB (git's default
  `core.bigFileThreshold`), comparing contents would need both versions in memory, so radar only
  pairs exact copies and otherwise counts the deletion.

## Limitations

* **Stale file timestamps cost a rehash on every prompt.** After `touch`, or a tool that
  rewrites files without changing them, radar rehashes those files to see that they're
  unchanged (about 0.8 s per GiB). `git status` does the same once and records the result in
  the index; radar never writes the index, so it pays again on every prompt until some git
  command refreshes it. Past ~6 GiB of such files that exceeds the 5 s limit and the prompt is
  empty. Run `git status` once to fix it.
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
* **Nested submodules are scanned 16 levels deep.** Each level is a full status run. Below
  that, a submodule still shows ` M` when its HEAD moved, but changes inside it aren't seen.
  git has no such limit.
* **Untrusted repositories print nothing**, like git's "dubious ownership" refusal. That covers
  repos owned by another user and not allowed by `safe.directory`.
* **bash only.** There's no `--zsh`/`--fish`, no `--fetch`, and no `GIT_RADAR_*` or `.gitradarrc` customization.

### Scope of "no subprocesses"

What's tested: radar never spawns `git`. A test replaces `git` on `PATH` with a logging shim.
Another test points every helper git can run at a logging script: filter drivers (also with
mixed-case section names and `required = true`), `diff.*.textconv`, `diff.*.command`,
`diff.external`, merge drivers, `core.fsmonitor`, hooks, `core.sshCommand`, the pager, askpass,
the editor, credential helpers and `gpg.program`. It runs radar on a clean tree, stale
timestamps, modified files, a staged rename, intent-to-add plus deletion, untracked files and a
dirty submodule, and asserts that no helper ran and the output is the same as without them.

This is a test, not a sandbox. The binary contains gitoxide's code for running filter drivers
and textconv; radar keeps it from running by removing `filter.*` config and by never asking
gitoxide for text conversion.

## Development

```bash
cargo test                       # unit + golden tests (golden needs git-radar installed)
cargo test -- --ignored          # also the >40k-object, 1 GiB memory-cap and huge-file tests
GIT_RADAR_BIN=/path/to/git-radar cargo test
scripts/bench.sh <repo-dir>      # hyperfine: git-radar vs radar vs `git status`
cargo deny --locked check advisories   # RustSec advisories for Cargo.lock (needs cargo-deny)
```

Without git-radar the golden tests skip themselves. Set `RADAR_REQUIRE_ORACLE=1` to make them
fail instead. CI (`.github/workflows/ci.yml`, macOS) sets it and runs fmt, clippy, and the debug,
release and ignored tests. A separate job checks `Cargo.lock` against the RustSec advisory
database on every push and weekly, so a new advisory fails CI without a code change.

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

## License

[MIT](LICENSE)
