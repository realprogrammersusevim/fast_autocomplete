#!/bin/zsh
# Harvests completions for a single command via real zsh compsys.
# Outputs NDJSON to stdout: one object per completion node.
#
# Architecture: spawn an interactive zsh inside a zpty, override compadd
# to capture matches instead of inserting them into a line editor display,
# then drive completion by typing each command path followed by TAB.
# Real compsys (compinit, _arguments, _describe, _files, ...) runs unmodified,
# so dispatch (e.g. _git -> _git-commit) and spec parsing are accurate.

emulate -L zsh
setopt extendedglob nullglob no_aliases pipefail
zmodload zsh/zpty 2>/dev/null || { print -u2 "zpty module unavailable"; exit 1; }

local TARGET=${1:-}
[[ -z $TARGET ]] && { print -u2 "usage: harvester.zsh <command>"; exit 1; }
[[ $TARGET == -* ]] && exit 0
[[ $TARGET == *[\ \(\)\[\]\{\}]* ]] && exit 0
# Skip commands the shell doesn't know about. Otherwise zsh's fallback
# _complete still emits a generic flag list, producing misleading output
# for misspellings / removed tools.
whence -- "$TARGET" >/dev/null 2>&1 || exit 0

local COMPDUMP="${TMPDIR:-/tmp}/fast_ac_compdump_zpty"
local MAX_ITEMS_PER_NODE=500
# Wall-clock budget. Slow completers (`_man` enumerating every command on PATH,
# `_brew` listing every formula) can spend several seconds inside a single
# `zpty -r`, which is uninterruptible — SIGALRM/TRAPALRM don't fire while it
# blocks. Cooperative checks between probes (see _hv_node) bound the recursive
# fan-out. As a hard backstop, fork a watchdog that SIGKILLs us if even the
# first probe never returns.
local MAX_ELAPSED_SECONDS=15

(
    sleep $MAX_ELAPSED_SECONDS
    kill -KILL $$ 2>/dev/null
) &!

typeset -gA _HV_SEEN
typeset -gi _HV_DEADLINE=$(( SECONDS + MAX_ELAPSED_SECONDS ))

_hv_json_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    print -rn -- "$s"
}

_hv_json_arr() {
    local -aU items=("$@")
    items=("${(o)items[@]}")
    local out="[" first=1 item
    for item in "${items[@]}"; do
        [[ -z $item ]] && continue
        (( first )) || out+=","
        first=0
        out+="\"$(_hv_json_escape "$item")\""
    done
    out+="]"
    print -rn -- "$out"
}

# ---- worker setup ----------------------------------------------------------
# Spawn one persistent interactive zsh; route all probes through it.
# Disable bracketed-paste (-z) ahead of time via TERM=dumb to keep the capture
# stream free of escape sequences.
TERM=dumb zpty -b _hv zsh -if 2>/dev/null
sleep 0.2

# Send a setup chunk and sync on a unique sentinel before continuing.
# macOS PTY input buffers cap around 1KB; writes that overflow are silently
# dropped under non-blocking mode (-b above), so the worker would never
# reach the rest of the setup. Sending in sub-1KB chunks with intermediate
# reads keeps the pipeline drained and guarantees full delivery.
typeset -gi _HV_SYNC=0
_hv_send_sync() {
    local payload=$1
    (( _HV_SYNC++ ))
    local mark="__HV_SYNC_${_HV_SYNC}__"
    zpty -w _hv "${payload}; builtin print ${mark}"
    local _drain=""
    zpty -r _hv _drain "*${mark}*" || return 1
    return 0
}

_hv_send_sync "PROMPT='' RPROMPT='' PS2='' SPROMPT=''" || exit 1
_hv_send_sync "unsetopt BEEP LIST_BEEP CORRECT CORRECT_ALL AUTO_LIST AUTO_MENU MENU_COMPLETE LIST_AMBIGUOUS PROMPT_CR PROMPT_SP" || exit 1
# Strip bracketed-paste bindings so a fast TAB-after-space isn't interpreted
# as a paste-block start. ZLE itself stays enabled — TAB needs it to fire
# the completion widget.
_hv_send_sync "bindkey -r '^[[200~' 2>/dev/null; bindkey -r '^[[201~' 2>/dev/null" || exit 1
_hv_send_sync "zstyle ':completion:*' menu no; zstyle ':completion:*' list-prompt ''; zstyle ':completion:*' select-prompt ''; zstyle ':completion:*' insert-tab false" || exit 1
_hv_send_sync "autoload -Uz compinit && compinit -d ${(qqq)COMPDUMP} -u 2>/dev/null" || exit 1

# compadd shim: intercepts all completion calls.
# - Tracks -f/-F flag → emits <<HV_FILE>> (avoids capturing filesystem listings).
# - Tracks -a flag → expands array names via dynamic scoping (${(@P)name}).
#   This handles git's __gitcomp pattern: compadd -a -- array, where `array`
#   is a local var in __gitcomp visible here via zsh's dynamic scoping.
# - Handles `-` or `--` as the option/items separator. _arguments and similar
#   completers actually use single `-` (e.g. `compadd -J grp -D arr - -E -F`),
#   so we MUST accept it — but we also have to skip the *values* of one-arg
#   flags like -J/-D/-M/etc., otherwise a value that happens to be "-" or
#   "--" terminates option parsing one step too early.
# - Outputs <<HV_CA:-- items...>> for non-file completions.
_hv_send_sync 'compadd() { local _hv_f=0 _hv_a=0 _hv_past=0 _hv_skip=0 _hv_i _hv_arg; local -a _hv_items _hv_args=("$@"); for (( _hv_i = 1; _hv_i <= ${#_hv_args}; _hv_i++ )); do _hv_arg=${_hv_args[_hv_i]}; if (( _hv_past )); then if (( _hv_a )); then _hv_items+=("${(@P)_hv_arg}"); else [[ -n $_hv_arg ]] && _hv_items+=("$_hv_arg"); fi; continue; fi; if (( _hv_skip )); then _hv_skip=0; continue; fi; case $_hv_arg in (--|-) _hv_past=1 ;; (-[fF]) _hv_f=1 ;; (-a) _hv_a=1 ;; (-[PSpsiIWdJVXxrRDFAEMOtykn]) _hv_skip=1 ;; esac; done; (( _hv_f )) && builtin print -- "<<HV_FILE>>" || builtin print -r -- "<<HV_CA:-- ${_hv_items[*]}>>"; }' || exit 1

_hv_send_sync '_files() { builtin print -- "<<HV_FILE>>"; return 0 }; _path_files() { builtin print -- "<<HV_FILE>>"; return 0 }; _directories() { builtin print -- "<<HV_FILE>>"; return 0 }; _globbed_files() { builtin print -- "<<HV_FILE>>"; return 0 }' || exit 1

_hv_send_sync '_file_absolute() { builtin print -- "<<HV_FILE>>"; return 0 }; _absolute_path() { builtin print -- "<<HV_FILE>>"; return 0 }; _message() { return 0 }; _users() { return 0 }; _hosts() { return 0 }; _groups() { return 0 }; _pids() { return 0 }; _process_names() { return 0 }' || exit 1

# ---- probe one command path -----------------------------------------------
# Sends "<words> <TAB><Ctrl-U>print __HV_MARK_<n>__\n" and reads back until
# the mark appears. zsh's zpty -r caps its read at 1MB so this is bounded
# even when a slow completer (e.g. _man enumerating every command on PATH)
# never emits the mark; the parser-side size guard then drops the noise.
typeset -g _HV_RAW=""
typeset -g _HV_MARK=0

_hv_probe() {
    local buf=$1
    (( _HV_MARK++ ))
    local mark="__HV_MARK_${_HV_MARK}__"
    zpty -w -n _hv "$buf"$'\t'
    sleep 0.08
    zpty -w -n _hv $'\x15'
    zpty -w _hv "print $mark"
    _HV_RAW=""
    # Blocking read; zsh's zpty -r caps at 1MB so this can never read forever
    # even when a completer (e.g. _man) emits hundreds of KB and the mark gets
    # buried. The parser-side payload guard in _hv_parse_ca then skips any
    # capture larger than 8KB, so we don't recurse on noisy enumerations.
    zpty -r _hv _HV_RAW "*${mark}*" 2>/dev/null
}

# ---- parse one compadd capture ---------------------------------------------
# Decode "<<HV_CA:-- items...>>" into items appended to the named arrays.
# The shim normalises all compadd output to this format: everything after
# the -- separator is either a flag (starts with -) or a subcommand.
# Args: ca_args_string flags_array_name subs_array_name
#
# Bails out on huge payloads (man-style "every command on PATH" dumps run to
# hundreds of KB and recursing into each "subcommand" is catastrophic).
# A normal subcommand/flag list is well under 4KB.
_hv_parse_ca() {
    local args_str=$1 flagsvar=$2 subsvar=$3
    (( ${#args_str} > 8192 )) && return
    local -a parts=("${(@z)args_str}")
    local past=0 v
    # Per-call item budget — prevents pathological completers from filling
    # the parent's arrays beyond what MAX_ITEMS_PER_NODE will keep anyway.
    local -i added=0 cap=$(( MAX_ITEMS_PER_NODE + 50 ))
    for v in "${parts[@]}"; do
        (( added >= cap )) && return
        if (( past )); then
            [[ -z $v ]] && continue
            # __gitcomp emits this placeholder; not a real flag.
            [[ $v == '--no-...'* ]] && continue
            # Strip trailing space that __gitcomp appends to each flag.
            v=${v% }
            [[ -z $v ]] && continue
            if [[ $v == -* ]]; then
                eval "$flagsvar+=(\"\$v\")"
            else
                eval "$subsvar+=(\"\$v\")"
            fi
            (( added++ ))
        elif [[ $v == -- || $v == - ]]; then
            past=1
        fi
    done
}

# ---- harvest one node ------------------------------------------------------
_hv_node() {
    local -a cmd_words=("$@")
    local depth=${#cmd_words}
    (( depth > 3 )) && return
    (( SECONDS >= _HV_DEADLINE )) && return

    local key="${(j:>:)cmd_words}"
    [[ -n ${_HV_SEEN[$key]} ]] && return
    _HV_SEEN[$key]=1

    local joined="${(j: :)cmd_words}"
    local -a flags subs
    local wants=0 line probe count payload
    # Two primary probes: trailing space (subcommands / files) and trailing
    # single-dash (covers _arguments-style completers like _grep, _ls — they
    # emit BOTH short and long flags on `-`).
    for probe in "$joined " "$joined -"; do
        _hv_probe "$probe"
        count=0
        while IFS= read -r line; do
            line=${line%$'\r'}
            case $line in
                *'<<HV_FILE>>'*) [[ $probe == *' ' ]] && wants=1 ;;
                *'<<HV_CA:'*'>>'*)
                    payload=${line#*<<HV_CA:}
                    payload=${payload%%>>*}
                    _hv_parse_ca "$payload" flags subs
                    (( count++ ))
                    (( count > 64 )) && break
                    ;;
            esac
        done <<< "$_HV_RAW"
    done
    # Fallback: commands like grep whose first positional is a non-file slot
    # (e.g. `_guard "^-*" pattern`) won't dispatch to _files on the trailing-
    # space probe. Advance one positional with a placeholder to surface the
    # file-accepting slot. Only at depth 1, and only when we already have flags
    # but no subcommands and no file marker — otherwise we'd risk treating the
    # placeholder as a subcommand for git-style dispatchers.
    if (( depth == 1 && ! wants && ${#flags} > 0 && ${#subs} == 0 )); then
        _hv_probe "$joined x "
        while IFS= read -r line; do
            line=${line%$'\r'}
            [[ $line == *'<<HV_FILE>>'* ]] && { wants=1; break; }
        done <<< "$_HV_RAW"
    fi

    # Fallback: bash-completion-derived completers (homebrew's _git) only emit
    # long flags via __gitcomp_builtin when current word matches `--*`; a `-`
    # probe falls through to file completion. If we got no flags from `-`, try
    # `--` to recover. Adds zero probes for well-behaved _arguments completers
    # (the common case) since they already returned flags for `-`.
    if (( ${#flags} == 0 )); then
        _hv_probe "$joined --"
        count=0
        while IFS= read -r line; do
            line=${line%$'\r'}
            case $line in
                *'<<HV_CA:'*'>>'*)
                    payload=${line#*<<HV_CA:}
                    payload=${payload%%>>*}
                    _hv_parse_ca "$payload" flags subs
                    (( count++ ))
                    (( count > 64 )) && break
                    ;;
            esac
        done <<< "$_HV_RAW"
    fi

    # Cap each list to avoid pathological output.
    if (( ${#flags} > MAX_ITEMS_PER_NODE )); then
        flags=("${flags[@]:0:$MAX_ITEMS_PER_NODE}")
    fi
    if (( ${#subs} > MAX_ITEMS_PER_NODE )); then
        subs=("${subs[@]:0:$MAX_ITEMS_PER_NODE}")
    fi

    if (( ! wants && ${#flags} == 0 && ${#subs} == 0 )); then
        return
    fi

    local path_json="[" first=1 w
    for w in "${cmd_words[@]}"; do
        (( first )) || path_json+=","
        first=0
        path_json+="\"$(_hv_json_escape "$w")\""
    done
    path_json+="]"

    local wants_str=false
    (( wants )) && wants_str=true

    print -r -- "{\"path\":${path_json},\"flags\":$(_hv_json_arr "${flags[@]}"),\"subcommands\":$(_hv_json_arr "${subs[@]}"),\"wants_files\":${wants_str}}"

    if (( ${#subs} > 0 && depth < 3 )); then
        local -aU unique_subs=("${subs[@]}")
        local sub
        for sub in "${unique_subs[@]}"; do
            [[ -z $sub ]] && continue
            [[ $sub == -* ]] && continue
            # Skip recursion into things that obviously aren't subcommands:
            # git refs (HEAD, ORIG_HEAD, refs/foo, origin/main, @{u}), paths,
            # all-uppercase tokens. These come back when the parent completer
            # offers branch/file values rather than literal subcommand words.
            # Recursing into them produces another branch listing and burns
            # the wall-clock budget on useless work.
            case $sub in
                HEAD|ORIG_HEAD|FETCH_HEAD|MERGE_HEAD|CHERRY_PICK_HEAD|REVERT_HEAD) continue ;;
                */*|*@*|*~*|*\^*) continue ;;
                [A-Z_][A-Z_]##) continue ;;
            esac
            _hv_node "${cmd_words[@]}" "$sub"
        done
    fi
}

_hv_node "$TARGET"

zpty -d _hv 2>/dev/null
