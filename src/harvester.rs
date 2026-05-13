use std::io::{BufRead, BufReader};
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
    ".", ":", "[", "alias", "autoload", "bg", "bindkey", "break", "builtin", "bye",
    "cd", "chdir", "command", "compctl", "compdump", "compfiles", "compgroups",
    "compinit", "complist", "compquote", "comptags", "comptry", "compvalues",
    "continue", "declare", "dirs", "disable", "disown", "echo", "echotc", "echoti",
    "emulate", "enable", "eval", "exec", "exit", "export", "false", "fc", "fg",
    "float", "functions", "getln", "getopts", "glob", "hash", "history",
    "integer", "jobs", "kill", "let", "limit", "local", "log", "logout",
    "popd", "print", "printf", "pushd", "pushln", "pwd", "read", "readonly",
    "rehash", "return", "sched", "set", "setopt", "shift", "source", "stat",
    "suspend", "test", "time", "times", "trap", "true", "ttyctl", "type",
    "typeset", "ulimit", "umask", "unalias", "unfunction", "unhash", "unlimit",
    "unset", "unsetopt", "vared", "wait", "whence", "where", "which", "zcompile",
    "zformat", "zle", "zmodload", "zparseopts", "zregexparse", "zstat", "zstyle",
];

/// Return the sorted list of all known command names without running any completion functions.
/// Intended to run once at startup in a spawn_blocking task.
pub fn list_commands() -> anyhow::Result<Vec<String>> {
    let script_path = std::env::temp_dir().join("fast_ac_list_cmds.zsh");
    std::fs::write(&script_path, LIST_COMMANDS_SCRIPT)?;

    let output = Command::new("zsh")
        .arg(&script_path)
        .stderr(Stdio::null())
        .output()?;

    let mut cmds: Vec<String> = BufReader::new(output.stdout.as_slice())
        .lines()
        .filter_map(|l| l.ok())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .chain(ZSH_BUILTINS.iter().map(|s| s.to_string()))
        .collect();
    cmds.sort_unstable();
    cmds.dedup();
    log::info!("listed {} known commands", cmds.len());
    Ok(cmds)
}

/// Harvest completions for a single command and insert the results into `tree`.
/// Passes the command name as `$1` to the harvester script (JIT mode).
/// Intended to run in a spawn_blocking task.
pub fn harvest_command(cmd: &str, tree: &CompletionTree) -> anyhow::Result<()> {
    let script_path = std::env::temp_dir().join("fast_ac_harvester.zsh");
    std::fs::write(&script_path, HARVESTER_SCRIPT)?;

    // KillOnDrop ensures the child is killed and waited on every exit path
    // (normal return, early error, panic), preventing zombie processes.
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let mut child = KillOnDrop(
        Command::new("zsh")
            .arg(&script_path)
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );

    let stdout = child
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

    // child guard drops here: kills if still running, then waits to reap.
    drop(child);
    log::debug!("harvested {} nodes for '{}'", count, cmd);
    Ok(())
}
