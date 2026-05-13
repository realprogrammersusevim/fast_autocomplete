# fast_autocomplete.plugin.zsh
# Low-latency zsh tab completion via the fast_autocomplete daemon.
#
# Plugin managers (zinit, antidote, oh-my-zsh, etc.) source this file.
# Manual use: `source /path/to/fast_autocomplete.plugin.zsh` in ~/.zshrc

# --------------------------------------------------------------------------- #
#  Internal helpers                                                             #
# --------------------------------------------------------------------------- #

# Capture plugin directory at source time — $0 is reliable here but not inside
# functions called later from subshells (where %x/%0 lose their file context).
_FA_PLUGIN_DIR=${0:A:h}

# Launch the daemon detached from the current shell.
# Uses a subshell so the backgrounded process is reparented to PID 1 when the
# subshell exits — no job-control dependency, works in interactive and
# non-interactive shells alike.
_fa_launch() {
  ( nohup "$1" </dev/null &>/dev/null & )
}

_fa_socket_path() {
  if [[ -n ${FAST_AUTOCOMPLETE_SOCKET:-} ]]; then
    print -- "$FAST_AUTOCOMPLETE_SOCKET"
    return
  fi
  local uid=${UID:-$(id -u)}
  # TMPDIR on macOS ends with /, so use ${TMPDIR:-/tmp/} to always get a slash.
  print -- "${TMPDIR:-/tmp/}fast_autocomplete_${uid}.sock"
}

_fa_bin_path() {
  # 1. Explicit override.
  if [[ -n ${FAST_AUTOCOMPLETE_BIN:-} ]]; then
    print -- "$FAST_AUTOCOMPLETE_BIN"
    return
  fi
  # 2. Binary on PATH.
  if (( $+commands[fast_autocomplete] )); then
    print -- "fast_autocomplete"
    return
  fi
  # 3. Sibling target/release/ (running from the source tree).
  local release_bin=$_FA_PLUGIN_DIR/target/release/fast_autocomplete
  if [[ -x $release_bin ]]; then
    print -- "$release_bin"
  fi
}

_fa_ensure_daemon() {
  local sock bin
  sock=$(_fa_socket_path)
  [[ -S $sock ]] && return 0

  bin=$(_fa_bin_path)
  [[ -z $bin ]] && return 1

  _fa_launch "$bin"

  local i
  for i in {1..10}; do
    [[ -S $sock ]] && return 0
    sleep 0.1
  done
  return 1
}

# Expand the first word as an alias before querying the daemon.
# Only expands when the first word is complete (followed by a space).
# Skips aliases that contain shell metacharacters.
# Outputs expanded buffer length (new cursor), then expanded buffer.
_fa_alias_expand() {
  local buffer=$1 cursor=$2
  local before="${buffer:0:$cursor}" after="${buffer:$cursor}"
  local first_word="${before%% *}"

  # Only expand when the command word is done (space follows it).
  if [[ $before != $first_word' '* ]]; then
    print -- "$cursor"
    print -r -- "$buffer"
    return
  fi

  local expansion="${aliases[$first_word]-}"
  # Skip complex aliases (pipes, redirects, subshells, etc.).
  if [[ -z $expansion || $expansion == *['|&;()``$<>']* ]]; then
    print -- "$cursor"
    print -r -- "$buffer"
    return
  fi

  local rest="${before#$first_word}"
  local new_before="${expansion}${rest}"
  print -- "${#new_before}"
  print -r -- "${new_before}${after}"
}

# Fire-and-forget: tell the daemon a completion was accepted (for frecency).
_fa_record() {
  local value=$1 sock
  [[ -z $value ]] && return
  sock=$(_fa_socket_path)
  [[ ! -S $sock ]] && return
  local payload
  payload=$(printf 'RECORD=%s\n\n' "$value")
  if (( $+commands[socat] )); then
    ( print -- "$payload" | socat -t1 - "UNIX-CONNECT:$sock" &>/dev/null ) &!
  else
    ( print -- "$payload" | nc -U "$sock" &>/dev/null ) &!
  fi
}

# Send one request to the daemon and print the raw JSON response.
# Prefers socat, falls back to nc -U (both support Unix domain sockets).
_fa_query() {
  local sock=$1 buffer=$2 cursor=$3 cwd=$4 session=$5
  local payload
  payload=$(printf 'BUFFER=%s\nCURSOR=%d\nCWD=%s\nSESSION=%d\n\n' \
    "$buffer" "$cursor" "$cwd" "$session")

  if (( $+commands[socat] )); then
    print -- "$payload" | socat -t1 - "UNIX-CONNECT:$sock" 2>/dev/null
  else
    # nc -U works on macOS and most Linux distributions.
    print -- "$payload" | nc -U "$sock" 2>/dev/null
  fi
}

# --------------------------------------------------------------------------- #
#  Completion function                                                          #
# --------------------------------------------------------------------------- #

# Last completions returned by the daemon; reused on `unchanged:true`.
typeset -ga _FA_LAST_COMPLETIONS

_fast_autocomplete() {
  local sock response
  sock=$(_fa_socket_path)

  if [[ ! -S $sock ]]; then
    _fa_ensure_daemon || return 1
    [[ -S $sock ]] || return 1
  fi

  local -a _fa_exp
  _fa_exp=( ${(f)"$(_fa_alias_expand "$BUFFER" "$CURSOR")"} )
  local _fa_cursor=${_fa_exp[1]} _fa_buffer=${_fa_exp[2]}

  response=$(_fa_query "$sock" "$_fa_buffer" "$_fa_cursor" "$PWD" "$$")
  [[ -z $response ]] && return 1

  if [[ $response == *'"unchanged":true'* ]]; then
    (( ${#_FA_LAST_COMPLETIONS} == 0 )) && return 1
    compadd -V fast_autocomplete -Q -U -- "${_FA_LAST_COMPLETIONS[@]}"
    if [[ $_FA_REVERSE_COMPLETE == 1 ]]; then
      compstate[insert]='menu:-1'
    else
      compstate[insert]='menu:1'
    fi
    return 0
  fi

  local -a completions
  if (( $+commands[jq] )); then
    completions=( ${(f)"$(jq -r '.completions[]? // empty' <<< "$response" 2>/dev/null)"} )
  else
    # Minimal fallback for simple completions (no embedded quotes or commas).
    local raw=${response#*'"completions":['}
    raw=${raw%%\]*}
    completions=( ${(s:,:)${raw//\"/}} )
  fi

  (( ${#completions} == 0 )) && return 1

  _FA_LAST_COMPLETIONS=( "${completions[@]}" )

  # -Q: don't quote special characters zsh would add
  # -U: skip zsh's own prefix filter (daemon already filtered)
  # -V: unsorted group — preserves daemon's ranked order so menu:1 inserts the top result
  compadd -V fast_autocomplete -Q -U -- "${completions[@]}"

  if [[ $_FA_REVERSE_COMPLETE == 1 ]]; then
    compstate[insert]='menu:-1'
  else
    compstate[insert]='menu:1'
  fi
}

# --------------------------------------------------------------------------- #
#  As-you-type display (zle -M area below the prompt)                          #
# --------------------------------------------------------------------------- #

typeset -g _FA_PREV_BUFFER_DISPLAY=''
typeset -ga _FA_LAST_DISPLAY_COMPLETIONS

_fa_update_below() {
  # Skip when there are pending keystrokes — avoids blocking mid-rapid-type
  (( PENDING )) && return
  # Skip when nothing has changed (cursor moves, redraws, etc.)
  [[ $BUFFER == $_FA_PREV_BUFFER_DISPLAY ]] && return
  _FA_PREV_BUFFER_DISPLAY=$BUFFER

  if [[ -z ${BUFFER// } ]]; then
    zle -M ''
    return
  fi

  local sock response
  sock=$(_fa_socket_path)
  if [[ ! -S $sock ]]; then
    zle -M ''
    return
  fi

  local -a _fa_exp
  _fa_exp=( ${(f)"$(_fa_alias_expand "$BUFFER" "$CURSOR")"} )
  local _fa_cursor=${_fa_exp[1]} _fa_buffer=${_fa_exp[2]}

  response=$(_fa_query "$sock" "$_fa_buffer" "$_fa_cursor" "$PWD" "$(( $$ + 1000000 ))")
  [[ -z $response ]] && return

  local -a completions

  # unchanged:true means the daemon's cached list is current — re-render it.
  # The display may have been cleared by _fa_clear_below even though the list
  # hasn't changed (e.g. user ran a command then retyped the same input).
  if [[ $response == *'"unchanged":true'* ]]; then
    (( ${#_FA_LAST_DISPLAY_COMPLETIONS} == 0 )) && return
    completions=( "${_FA_LAST_DISPLAY_COMPLETIONS[@]}" )
  else
    if (( $+commands[jq] )); then
      completions=( ${(f)"$(jq -r '.completions[]? // empty' <<< "$response" 2>/dev/null)"} )
    else
      local raw=${response#*'"completions":['}
      raw=${raw%%\]*}
      completions=( ${(s:,:)${raw//\"/}} )
    fi

    if (( ${#completions} == 0 )); then
      zle -M ''
      return
    fi

    _FA_LAST_DISPLAY_COMPLETIONS=( "${completions[@]}" )
  fi

  # Format completions into aligned columns, capped at max_rows display lines.
  local term_width=${COLUMNS:-80}
  local max_rows=8
  local max_shown=40
  local -a shown=( "${completions[@]:0:$max_shown}" )

  local max_len=0 c
  for c in "${shown[@]}"; do
    (( ${#c} > max_len )) && max_len=${#c}
  done

  local col_width=$(( max_len + 2 ))
  local num_cols=$(( term_width / col_width ))
  (( num_cols < 1 )) && num_cols=1

  # Recompute max_shown so we never exceed max_rows lines.
  local max_by_rows=$(( num_cols * max_rows ))
  (( max_by_rows < max_shown )) && max_shown=$max_by_rows
  shown=( "${completions[@]:0:$max_shown}" )

  local output='' i=0
  for c in "${shown[@]}"; do
    output+="${(r:$col_width:)c}"
    (( ++i % num_cols == 0 )) && output+=$'\n'
  done
  # Trim trailing newline and add overflow notice if needed.
  output=${output%$'\n'}
  (( ${#completions} > max_shown )) && output+=$'\n'"  … ($(( ${#completions} - max_shown )) more, press Tab to browse)"

  zle -M -- "$output"
}

_fa_clear_below() {
  # When the user accepts a line, record the last word if it matches what we
  # offered as a completion — this feeds the in-daemon frecency ranking.
  if (( ${#_FA_LAST_COMPLETIONS} > 0 && ${#BUFFER} > 0 )); then
    local -a _fa_words
    _fa_words=( ${(z)BUFFER} )
    local _fa_last=${_fa_words[-1]}
    if [[ -n ${_FA_LAST_COMPLETIONS[(r)${_fa_last}]} ]]; then
      _fa_record "$_fa_last"
    fi
  fi
  zle -M ''
  _FA_PREV_BUFFER_DISPLAY=''
}

autoload -Uz add-zle-hook-widget
add-zle-hook-widget zle-line-pre-redraw _fa_update_below
add-zle-hook-widget zle-line-finish     _fa_clear_below

# Shift+Tab: complete using the last (lowest-ranked) item in the list.
typeset -g _FA_REVERSE_COMPLETE=0
_fa_reverse_complete_widget() {
  _FA_REVERSE_COMPLETE=1
  zle expand-or-complete
  _FA_REVERSE_COMPLETE=0
}
zle -N _fa_reverse_complete_widget
bindkey '^[[Z' _fa_reverse_complete_widget

# --------------------------------------------------------------------------- #
#  Plugin setup                                                                 #
# --------------------------------------------------------------------------- #

# Register _fast_autocomplete as the first completer.
# On failure (daemon down, no completions) it returns non-zero and zsh falls
# through to normal _complete and then _files.
zstyle ':completion:*' completer _fast_autocomplete _complete _files

# Kick off the daemon directly (not via subshell) so disown takes effect in
# the current shell. _fa_ensure_daemon must not be called via & here.
if [[ ! -S $(_fa_socket_path) ]]; then
  _fa_plugin_bin=$(_fa_bin_path)
  if [[ -n $_fa_plugin_bin ]]; then
    _fa_launch "$_fa_plugin_bin"
  fi
  unset _fa_plugin_bin
fi
