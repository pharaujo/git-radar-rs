#!/usr/bin/env bash
# Benchmark radar against git-radar and plain `git status` in a repository.
#
#   scripts/bench.sh [repo-dir]    (default: current directory)
#
# Never installs anything: exits non-zero if a dependency is missing.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
radar="${RADAR_BIN:-$here/target/release/radar}"
oracle="${GIT_RADAR_BIN:-$(command -v git-radar || true)}"

missing=0
command -v hyperfine >/dev/null || { echo "missing: hyperfine (brew install hyperfine)" >&2; missing=1; }
[[ -n "$oracle" && -x "$oracle" ]] || { echo "missing: git-radar (or set GIT_RADAR_BIN)" >&2; missing=1; }
[[ -x "$radar" ]] || { echo "missing: $radar (run: cargo build --release)" >&2; missing=1; }
(( missing == 0 )) || exit 1

cd "${1:-.}"
git rev-parse --git-dir >/dev/null 2>&1 || { echo "not a git repository: $PWD" >&2; exit 1; }

echo "repo: $PWD"
echo "tracked files: $(git ls-files | wc -l | tr -d ' ')"
echo "index size: $(wc -c < "$(git rev-parse --git-path index)" | tr -d ' ') bytes"
for key in core.fsmonitor core.untrackedCache feature.manyFiles; do
  echo "$key: $(git config --get "$key" || echo '(unset)')"
done
echo

hyperfine -N --warmup 5 \
  --command-name git-radar "$oracle --bash" \
  --command-name radar "$radar --bash" \
  --command-name 'git status' 'git status --porcelain' \
  "${@:2}"
