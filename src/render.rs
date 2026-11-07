//! Byte-exact rendering of git-radar 0.6's default bash prompt (`prepare_bash_colors` plus
//! `render_prompt` in radar-base.sh).

use crate::status::Counts;

/// What the prompt shows. The fields are filled in by `repo::gather`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct State {
    /// Branch name, or `detached@<short sha>`.
    pub branch: Vec<u8>,
    /// `None` means no upstream configured, rendered as `upstream ⚡`.
    pub upstream: Option<Upstream>,
    pub stash: usize,
    /// `None` if there is no worktree to compare against.
    pub changes: Option<Counts>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Upstream {
    /// Local commits not on the upstream.
    pub ahead: usize,
    /// Upstream commits not in HEAD.
    pub behind: usize,
    /// Upstream commits not on the main branch.
    pub main_ahead: usize,
    /// Main-branch commits not on the upstream.
    pub main_behind: usize,
}

/// `\x01 ESC[<code>m \x02`, readline's non-printing markers around an SGR sequence.
fn c(out: &mut Vec<u8>, code: &str) {
    out.extend_from_slice(b"\x01\x1b[");
    out.extend_from_slice(code.as_bytes());
    out.extend_from_slice(b"m\x02");
}

fn colored(out: &mut Vec<u8>, code: &str, text: &str) {
    c(out, code);
    out.extend_from_slice(text.as_bytes());
    c(out, "0");
}

fn num(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(n.to_string().as_bytes());
}

const GREEN: &str = "1;32";
const RED: &str = "1;31";
const YELLOW: &str = "1;33";
const WHITE: &str = "1;37";
const GREY: &str = "1;30";

/// `color_remote_commits`
fn remote(state: &State) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(up) = state.upstream else {
        out.extend_from_slice(b"upstream ");
        colored(&mut out, RED, "⚡");
        return out;
    };
    let master = |out: &mut Vec<u8>| colored(out, "0", "𝘮");
    let (ahead, behind) = (up.main_ahead, up.main_behind);
    if behind > 0 && ahead > 0 {
        master(&mut out);
        out.push(b' ');
        num(&mut out, behind);
        out.push(b' ');
        colored(&mut out, YELLOW, "⇄");
        out.push(b' ');
        num(&mut out, ahead);
    } else if ahead > 0 {
        master(&mut out);
        out.push(b' ');
        colored(&mut out, GREEN, "←");
        out.push(b' ');
        num(&mut out, ahead);
    } else if behind > 0 {
        master(&mut out);
        out.push(b' ');
        num(&mut out, behind);
        out.push(b' ');
        colored(&mut out, RED, "→");
    }
    out
}

/// `color_local_commits`
fn local(state: &State) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(up) = state.upstream else { return out };
    if up.behind > 0 && up.ahead > 0 {
        num(&mut out, up.behind);
        colored(&mut out, YELLOW, "⇵");
        num(&mut out, up.ahead);
    } else if up.behind > 0 {
        num(&mut out, up.behind);
        colored(&mut out, RED, "↓");
    } else if up.ahead > 0 {
        num(&mut out, up.ahead);
        colored(&mut out, GREEN, "↑");
    }
    out
}

/// `stash_status`
fn stash(state: &State) -> Vec<u8> {
    let mut out = Vec::new();
    if state.stash > 0 {
        num(&mut out, state.stash);
        colored(&mut out, YELLOW, "≡");
    }
    out
}

/// `color_changes_status`: groups joined by a space, each group the concatenation of
/// `<count><color><letter><reset>` for non-zero counts.
fn changes(state: &State) -> Vec<u8> {
    let Some(ch) = &state.changes else {
        return Vec::new();
    };
    let groups: [(&str, &[(usize, &str)]); 4] = [
        (
            GREEN,
            &[
                (ch.staged_added, "A"),
                (ch.staged_deleted, "D"),
                (ch.staged_modified, "M"),
                (ch.staged_renamed, "R"),
                (ch.staged_copied, "C"),
            ],
        ),
        (
            YELLOW,
            &[
                (ch.conflict_us, "U"),
                (ch.conflict_them, "T"),
                (ch.conflict_both, "B"),
            ],
        ),
        (
            RED,
            &[(ch.unstaged_deleted, "D"), (ch.unstaged_modified, "M")],
        ),
        (WHITE, &[(ch.untracked, "A")]),
    ];
    let mut out = Vec::new();
    for (color, items) in groups {
        let mut group = Vec::new();
        for &(n, letter) in items {
            if n > 0 {
                num(&mut group, n);
                colored(&mut group, color, letter);
            }
        }
        if !group.is_empty() {
            if !out.is_empty() {
                out.push(b' ');
            }
            out.extend_from_slice(&group);
        }
    }
    out
}

/// The default `GIT_RADAR_FORMAT`:
/// ` <grey>git:(<reset>%{remote: }%{branch}%{ :local}<grey>)<reset>%{ :stash}%{ :changes}`
pub fn prompt(state: &State, out: &mut Vec<u8>) {
    out.push(b' ');
    c(out, GREY);
    out.extend_from_slice(b"git:(");
    c(out, "0");

    let r = remote(state);
    if !r.is_empty() {
        out.extend_from_slice(&r);
        out.push(b' ');
    }

    c(out, "0");
    out.extend_from_slice(&state.branch);
    c(out, "0");

    #[cfg(debug_assertions)]
    if std::env::var_os("RADAR_TEST_PANIC").is_some_and(|v| v == "after_branch") {
        panic!("RADAR_TEST_PANIC=after_branch");
    }

    let l = local(state);
    if !l.is_empty() {
        out.push(b' ');
        out.extend_from_slice(&l);
    }

    c(out, GREY);
    out.push(b')');
    c(out, "0");

    for segment in [stash(state), changes(state)] {
        if !segment.is_empty() {
            out.push(b' ');
            out.extend_from_slice(&segment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes captured from `git-radar --bash` 0.6: branch `feat`, 1 commit ahead of
    /// origin/master, in sync with its upstream, 1 stash, 1 unstaged modification,
    /// 2 untracked files.
    #[test]
    fn matches_captured_oracle_bytes() {
        let state = State {
            branch: b"feat".to_vec(),
            upstream: Some(Upstream {
                main_ahead: 1,
                ..Default::default()
            }),
            stash: 1,
            changes: Some(Counts {
                unstaged_modified: 1,
                untracked: 2,
                ..Default::default()
            }),
        };
        let mut out = Vec::new();
        prompt(&state, &mut out);
        let expected: &[u8] = b" \x01\x1b[1;30m\x02git:(\x01\x1b[0m\x02\x01\x1b[0m\x02\xf0\x9d\x98\xae\x01\x1b[0m\x02 \x01\x1b[1;32m\x02\xe2\x86\x90\x01\x1b[0m\x02 1 \x01\x1b[0m\x02feat\x01\x1b[0m\x02\x01\x1b[1;30m\x02)\x01\x1b[0m\x02 1\x01\x1b[1;33m\x02\xe2\x89\xa1\x01\x1b[0m\x02 1\x01\x1b[1;31m\x02M\x01\x1b[0m\x02 2\x01\x1b[1;37m\x02A\x01\x1b[0m\x02";
        assert_eq!(
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(expected)
        );
    }
}
