use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::cache::CompletionTree;

const HARVESTER_SCRIPT: &str = include_str!("../scripts/harvester.zsh");

/// Lightweight script: emits one command name per line from zsh's completion table.
/// No completion functions are called, so this completes in a few seconds.
const LIST_COMMANDS_SCRIPT: &str = r#"emulate -L zsh
setopt extendedglob nullglob no_aliases
autoload -Uz compinit
compinit -d "${TMPDIR:-/tmp}/fast_ac_compdump" -C 2>/dev/null
for cmd in ${(k)_comps}; do
    [[ -z $cmd ]] && continue
    [[ $cmd == -* ]] && continue
    [[ $cmd == *[\ \(\)\[\]\{\}]* ]] && continue
    print -r -- "$cmd"
done
"#;

#[derive(serde::Deserialize)]
struct HarvesterLine {
    path: Vec<String>,
    flags: Vec<String>,
    subcommands: Vec<String>,
    wants_files: bool,
}

const ZSH_BUILTINS: &[&str] = &[
    ".",
    ":",
    "[",
    "alias",
    "autoload",
    "bg",
    "bindkey",
    "break",
    "builtin",
    "bye",
    "cd",
    "chdir",
    "command",
    "compctl",
    "compdump",
    "compfiles",
    "compgroups",
    "compinit",
    "complist",
    "compquote",
    "comptags",
    "comptry",
    "compvalues",
    "continue",
    "declare",
    "dirs",
    "disable",
    "disown",
    "echo",
    "echotc",
    "echoti",
    "emulate",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "float",
    "functions",
    "getln",
    "getopts",
    "glob",
    "hash",
    "history",
    "integer",
    "jobs",
    "kill",
    "let",
    "limit",
    "local",
    "log",
    "logout",
    "popd",
    "print",
    "printf",
    "pushd",
    "pushln",
    "pwd",
    "read",
    "readonly",
    "rehash",
    "return",
    "sched",
    "set",
    "setopt",
    "shift",
    "source",
    "stat",
    "suspend",
    "test",
    "time",
    "times",
    "trap",
    "true",
    "ttyctl",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unfunction",
    "unhash",
    "unlimit",
    "unset",
    "unsetopt",
    "vared",
    "wait",
    "whence",
    "where",
    "which",
    "zcompile",
    "zformat",
    "zle",
    "zmodload",
    "zparseopts",
    "zregexparse",
    "zstat",
    "zstyle",
];

/// Return the sorted list of all known command names without running any completion functions.
/// Intended to run once at startup in a `spawn_blocking` task.
pub fn list_commands() -> anyhow::Result<Vec<String>> {
    let script_path = std::env::temp_dir().join("fast_ac_list_cmds.zsh");
    std::fs::write(&script_path, LIST_COMMANDS_SCRIPT)?;

    let output = Command::new("zsh")
        .arg(&script_path)
        .stderr(Stdio::null())
        .output()?;

    let mut cmds: Vec<String> = BufReader::new(output.stdout.as_slice())
        .lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .chain(ZSH_BUILTINS.iter().map(ToString::to_string))
        .collect();
    cmds.sort_unstable();
    cmds.dedup();
    log::info!("listed {} known commands", cmds.len());
    Ok(cmds)
}

/// Harvest completions for a single command and insert the results into `tree`.
/// Passes the command name as `$1` to the harvester script (JIT mode).
/// Intended to run in a `spawn_blocking` task.
pub fn harvest_command(cmd: &str, tree: &CompletionTree) -> anyhow::Result<()> {
    // KillOnDrop ensures every spawned process is reaped on every exit path
    // (normal return, early error, panic), preventing CPU-burning orphans.
    //
    // The process tree we spawn looks like:
    //   parent zsh (this Child) — own pgid via process_group(0) below
    //     ├─ wall-clock watchdog subshell (`&!`-disowned) — same pgid
    //     └─ zpty intermediate process — same pgid
    //         └─ `zsh -if` worker — OWN session via zpty's setsid()
    //
    // A plain `child.kill()` only signals the parent zsh; the worker's
    // controlling-tty hangup *usually* delivers SIGHUP, but a worker stuck
    // in a CPU-bound completion function won't notice until that function
    // returns — potentially never. Killpg on the parent's pgid catches the
    // watchdog but NOT the worker (different session).
    //
    // So we sweep the parent's child tree first (recursively SIGKILLing
    // every descendant, including the setsid'd worker), then killpg the
    // parent's pgid for any same-group stragglers, then reap the parent.
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            kill_tree(self.0.id() as i32);
            unsafe { libc::killpg(self.0.id() as i32, libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    // Hard cap on nodes ingested per command. Real-world harvests emit at most a
    // few hundred; anything larger is a pathological completion definition (or
    // a runaway recursion) and would balloon the tree without improving UX.
    // The KillOnDrop guard on `child` reaps the subprocess once we break.
    const MAX_NODES: usize = 5_000;

    let script_path = std::env::temp_dir().join("fast_ac_harvester.zsh");
    std::fs::write(&script_path, HARVESTER_SCRIPT)?;

    let mut guard = KillOnDrop(
        Command::new("zsh")
            .arg(&script_path)
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?,
    );

    let stdout = guard
        .0
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    let reader = BufReader::new(stdout);
    let mut count = 0usize;

    for line in reader.lines() {
        let line = match line {
            Ok(l) if !l.is_empty() => l,
            _ => continue,
        };
        match serde_json::from_str::<HarvesterLine>(&line) {
            Ok(entry) => {
                tree.insert(
                    &entry.path,
                    entry.flags,
                    entry.subcommands,
                    entry.wants_files,
                );
                count += 1;
                if count >= MAX_NODES {
                    log::warn!("harvest '{cmd}' hit {MAX_NODES}-node cap; truncating");
                    break;
                }
            }
            Err(e) => {
                log::debug!(
                    "harvester parse error: {} — line: {}",
                    e,
                    &line[..line.len().min(80)]
                );
            }
        }
    }

    // Guard drops here: sweeps descendants, killpg's the parent's group,
    // then reaps the parent.
    drop(guard);
    log::debug!("harvested {count} nodes for '{cmd}'");
    Ok(())
}

/// Recursively SIGKILL all descendants of `pid` (children, grandchildren, ...).
/// Used by `KillOnDrop` to reach grandchildren in their own process sessions —
/// notably the `zpty` worker, which `setsid()`s itself out of the parent's
/// process group and so escapes a plain `killpg(parent_pgid)`.
///
/// Uses `pgrep -P` because no portable libc call enumerates children, and
/// walking `/proc` doesn't work on macOS. `pgrep` is in `/usr/bin` on macOS
/// and base on every Linux distro that ships a daemon like this.
fn kill_tree(pid: i32) {
    let output = match Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(_) => return,
    };
    for line in output.stdout.split(|&b| b == b'\n') {
        let s = match std::str::from_utf8(line) {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };
        if let Ok(child) = s.parse::<i32>() {
            // Recurse first so leaves die before we kill the parent that
            // would otherwise re-parent them to init mid-sweep.
            kill_tree(child);
            unsafe { libc::kill(child, libc::SIGKILL) };
        }
    }
}
