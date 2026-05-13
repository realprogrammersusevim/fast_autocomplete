#!/bin/zsh
# Harvests completions for all commands known to zsh's completion system.
# Outputs NDJSON to stdout: one object per completion node.
# Called once at daemon startup.

emulate -L zsh
setopt extendedglob nullglob no_aliases

autoload -Uz compinit
compinit -d "${TMPDIR:-/tmp}/fast_ac_compdump" -C 2>/dev/null

typeset -gA _HV_SEEN

_hv_json_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    print -rn -- "$s"
}

_hv_json_arr() {
    local -aU items=("$@")
    local out="["
    local first=1
    local item
    for item in "${items[@]}"; do
        [[ -z $item ]] && continue
        (( first )) || out+=","
        first=0
        out+="\"$(_hv_json_escape "$item")\""
    done
    out+="]"
    print -rn -- "$out"
}

# Run a completion function and capture its output, with a hard timeout.
# Prints tagged lines: FLAG:<name>, ITEM:<name>, FILE:
# Args: comp_func cmd_word [subcmd_word...]
_hv_run_func() {
    local comp_func=$1
    shift
    local -a cmd_words=("$@")
    local top="${cmd_words[1]}"

    # Run in a subshell for isolation. ALARM=1 + TRAPALRM inside interrupts
    # the completion function after 1 second without spawning extra processes.
    (
        # compadd: intercept items being added to the completion list.
        compadd() {
            local f=0 skip=0 past=0 i a
            for (( i = 1; i <= $#; i++ )); do
                a=${@[$i]}
                (( skip )) && { skip=0; continue; }
                if (( past )); then
                    [[ -n $a ]] && print -r -- "ITEM:${a}"
                    continue
                fi
                case $a in
                    --) past=1 ;;
                    -f|-F) f=1 ;;
                    -a)
                        (( i++ ))
                        local -a _arr=("${(@P)${@[$i]}}")
                        for x in "${_arr[@]}"; do
                            [[ -n $x ]] && print -r -- "ITEM:${x}"
                        done
                        ;;
                    -k)
                        (( i++ ))
                        local -a _keys=("${(@Pk)${@[$i]}}")
                        for x in "${_keys[@]}"; do
                            [[ -n $x ]] && print -r -- "ITEM:${x}"
                        done
                        ;;
                    # Options that consume the next argument
                    -d|-J|-V|-X|-M|-P|-S|-r|-R|-p|-s|-o|-W|-i|-I|-t|-n|-e|-2|-1|-l|-E|-O|-q|-Q|-U|-L) skip=1 ;;
                esac
            done
            (( f )) && print -- "FILE:"
        }

        # _arguments: parse specs to extract flags, literal value lists, and state names.
        # When -C is present, set $state only for the ->state spec whose positional
        # index matches CURRENT so the caller dispatches to the right handler.
        # Return 1 when state is set so that "... && return" guards in callers don't fire.
        _arguments() {
            local _has_C=0 spec bare flag action list v
            # Argument position being completed (1 = first arg after command name).
            local _cur_pos=$(( CURRENT - 1 ))
            local _seq=0  # counter for unnumbered positional specs
            [[ ${@[(r)-C]} == -C ]] && _has_C=1

            for spec in "$@"; do
                [[ $spec == -[sSCwARO] ]] && continue
                [[ $spec == -- ]] && continue

                bare="$spec"
                [[ $bare == \(*\)* ]] && bare="${bare#\(*\)}"
                [[ $bare == [+\!]* ]] && bare="${bare#[+\!]}"

                if [[ $bare == [-+]* ]]; then
                    flag="${bare%%[\[:= ]*}"
                    [[ $flag == -* ]] && [[ ${#flag} -ge 2 ]] && print -r -- "FLAG:${flag}"
                fi

                if [[ $spec == *:* ]]; then
                    action="${spec##*:}"

                    # Resolve whether this positional spec applies at _cur_pos.
                    local _is_pos=0 _pos_match=0
                    if [[ $bare != [-+]* ]]; then
                        _is_pos=1
                        local _head="${bare%%:*}"
                        if [[ $_head == <-> ]]; then
                            # Explicitly numbered: '1:msg:action'
                            (( _head == _cur_pos )) && _pos_match=1
                        elif [[ $_head == '#' ]]; then
                            # Numeric argument: treat like unnumbered positional
                            (( ++_seq == _cur_pos )) && _pos_match=1
                        elif [[ $_head == '*' || $_head == '**' ]]; then
                            # Catch-all: matches any remaining position
                            _pos_match=1
                        else
                            # Unnumbered positional: '::msg:action' or ':msg:action'
                            (( ++_seq == _cur_pos )) && _pos_match=1
                        fi
                    fi

                    case $action in
                        _files|_path_files|_globbed_files|_absolute_path|_file_absolute)
                            print -- "FILE:" ;;
                        _directories|_dir_list|_path_dirs)
                            print -- "FILE:" ;;
                        \(*\))
                            list="${action#(}"
                            list="${list%)}"
                            for v in ${=list}; do
                                [[ -n $v ]] && print -r -- "ITEM:${v}"
                            done
                            ;;
                        -\>*)
                            # Only dispatch state machine for the spec that matches CURRENT.
                            if (( _has_C && _is_pos && _pos_match )) && [[ -z $state ]]; then
                                state="${action#->}"
                                # Emit FILE: now for catch-all positionals (head == * or **).
                                # Completion functions like _rm use '*:: :->file' then call
                                # _files in their case block, but that block is unreachable here
                                # because '_rm' re-declares 'line' as a scalar and the subsequent
                                # 'line[CURRENT]=()' assignment fatally exits the subshell before
                                # _files is ever called. Emitting FILE: inside _arguments ensures
                                # it is captured even when the caller exits early.
                                [[ $_head == '*' || $_head == '**' ]] && print -- "FILE:"
                            fi
                            ;;
                    esac
                fi
            done
            # Non-zero return prevents "... && return" guards from short-circuiting.
            (( _has_C && ${#state} > 0 )) && return 1
            return 0
        }

        _files()        { print -- "FILE:"; }
        _path_files()   { print -- "FILE:"; }
        _directories()  { print -- "FILE:"; }
        _globbed_files(){ print -- "FILE:"; }
        _pick_variant() { return 0; }
        _wanted()       { shift 2; "$@" 2>/dev/null; }
        _call_function(){ "$@" 2>/dev/null; }
        _tags()         { return 0; }
        _requested()    { return 0; }
        _next_label()   { return 1; }
        _prefix()       { return 0; }
        _suffix()       { return 0; }
        _multi_parts()  { return 0; }
        _values()       { return 0; }
        compset()       { return 0; }
        _regex_arguments(){ return 0; }
        _regex_words()  { return 0; }

        # _describe [-t tag] description array [array ...]
        # Each element of the named array is 'value:description' or just 'value'.
        _describe() {
            local skip=0 a
            for a in "$@"; do
                (( skip )) && { skip=0; continue; }
                case $a in
                    # Options that consume next arg
                    -t|-J|-V) skip=1; continue ;;
                    # Boolean flags
                    -[12oOnrxX]) continue ;;
                esac
                # Try to expand as an array variable; description strings won't match.
                local -a _da=("${(@P)a}")
                local item
                for item in "${_da[@]}"; do
                    [[ -n $item ]] && print -r -- "ITEM:${item%%:*}"
                done
            done
        }

        _alternative() {
            local spec action list v
            for spec in "$@"; do
                action="${spec##*:}"
                case $action in
                    \(*\))
                        list="${action#(}"; list="${list%)}";
                        for v in ${=list}; do [[ -n $v ]] && print -r -- "ITEM:${v}"; done ;;
                    _*|__*)
                        # Action may include arguments (e.g. "_path_files -/"), so split
                        # into function name + args before checking existence and calling.
                        local _alt_func="${action%% *}"
                        (( ${+functions[$_alt_func]} )) && ${=action} 2>/dev/null ;;
                esac
            done
        }

        _call_function() {
            shift  # skip return-value variable name
            (( ${+functions[$1]} )) || return 1
            "$@" 2>/dev/null
        }

        _retrieve_cache() { return 1; }
        _store_cache()    { return 0; }
        _cache_invalid()  { return 0; }

        # ALARM/TRAPALRM: interrupt the completion function if it takes too long.
        # This avoids spawning extra background processes for timeout management.
        TRAPALRM() { return 1; }
        ALARM=1

        local -a words=("${cmd_words[@]}" "")
        local CURRENT=$(( ${#cmd_words} + 1 ))
        local PREFIX='' SUFFIX='' IPREFIX='' ISUFFIX=''
        local curcontext=":complete:${top}:"
        local service="$top"
        local state state_descr
        local -a line
        local -A opt_args

        "$comp_func" 2>/dev/null
        ALARM=0
    ) 2>/dev/null
}

# Harvest the completion node for a given command path.
# Args: cmd [subcmd [subcmd...]]
_hv_node() {
    local -a cmd_words=("$@")
    local depth=${#cmd_words}
    (( depth > 3 )) && return

    local key="${(j:>:)cmd_words}"
    [[ -n ${_HV_SEEN[$key]} ]] && return
    _HV_SEEN[$key]=1

    local top="${cmd_words[1]}"
    local comp_func="${_comps[$top]:-_${top}}"

    if (( ! ${+functions[$comp_func]} )); then
        autoload -U "$comp_func" 2>/dev/null || return
        (( ${+functions[$comp_func]} )) || return
    fi

    local raw
    raw=$(_hv_run_func "$comp_func" "${cmd_words[@]}")

    local -a flags subs
    local wants=0 line
    while IFS= read -r line; do
        case $line in
            FILE:) wants=1 ;;
            FLAG:*)
                local v=${line#FLAG:}
                [[ -n $v ]] && flags+=("$v")
                ;;
            ITEM:*)
                local v=${line#ITEM:}
                [[ -z $v ]] && continue
                if [[ $v == -* ]]; then
                    flags+=("$v")
                else
                    subs+=("$v")
                fi
                ;;
        esac
    done <<< "$raw"

    local path_json="["
    local first=1
    local w
    for w in "${cmd_words[@]}"; do
        (( first )) || path_json+=","
        first=0
        path_json+="\"$(_hv_json_escape "$w")\""
    done
    path_json+="]"

    # If the completion function produced no flags, no subcommands, and no file
    # indicator, the harvest found nothing useful. Skip emitting a node so the
    # daemon falls back to file completions for unknown commands.
    if (( ! wants && ${#flags} == 0 && ${#subs} == 0 )); then
        return
    fi

    local wants_str=false
    (( wants )) && wants_str=true

    print -r -- "{\"path\":${path_json},\"flags\":$(_hv_json_arr "${flags[@]}"),\"subcommands\":$(_hv_json_arr "${subs[@]}"),\"wants_files\":${wants_str}}"

    if (( ${#subs} > 0 && depth < 3 )); then
        local -aU unique_subs=("${subs[@]}")
        local sub
        for sub in "${unique_subs[@]}"; do
            [[ -z $sub ]] && continue
            _hv_node "${cmd_words[@]}" "$sub"
        done
    fi
}

if [[ -n ${1:-} ]]; then
    # JIT mode: harvest a single command passed as $1.
    local cmd=$1
    [[ $cmd == -* ]] || [[ $cmd == *[\ \(\)\[\]\{\}]* ]] || _hv_node "$cmd"
else
    local cmd
    for cmd in ${(k)_comps}; do
        [[ -z $cmd ]] && continue
        [[ $cmd == -* ]] && continue
        [[ $cmd == *[\ \(\)\[\]\{\}]* ]] && continue
        _hv_node "$cmd"
    done
fi
