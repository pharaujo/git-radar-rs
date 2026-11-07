//! radar: a git-radar compatible prompt segment for bash, built on gitoxide.
//!
//! Output discipline: nothing ever goes to stderr, the exit code is always 0, and stdout is
//! written once, after the whole prompt has been built. A panic anywhere prints nothing, so a
//! half-written prompt with an unclosed `\x01` can't break readline.

mod graph;
mod installcfg;
mod mainbranch;
mod render;
mod repo;
mod status;

use std::io::Write;

const USAGE: &str = "\
radar - a heads up display for git, compatible with git-radar's bash output

usage:
  radar --bash

  --bash   # Output prompt using Bash style color characters

Bash example:
  export PS1=\"\\W\\$(radar --bash) \"

The main branch for the 𝘮 comparison is, in order: git config radar.mainBranch,
then <remote>/HEAD, <remote>/main, <remote>/master for the branch's remote and origin.
";

fn run(args: &[String]) -> Option<Vec<u8>> {
    if args.is_empty() {
        return Some(USAGE.as_bytes().to_vec());
    }
    if !args.iter().any(|a| a == "--bash") {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let state = repo::gather(&cwd)?;
    let mut out = Vec::with_capacity(256);
    render::prompt(&state, &mut out);
    Some(out)
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(Some(out)) = std::panic::catch_unwind(|| run(&args)) {
        let _ = std::io::stdout().write_all(&out);
    }
}
