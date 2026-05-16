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
_fa_launch() {
  ( nohup "$1" </dev/null &>/dev/null & )
}

_fa_socket_path() {
  if [[ -n ${FAST_AUTOCOMPLETE_SOCKET:-} ]]; then
    print -- "$FAST_AUTOCOMPLETE_SOCKET"
    return
  fi
  local uid=${UID:-$(id -u)}
  # Mirror src/main.rs::socket_path — stable user-scoped dir, not $TMPDIR.
  local dir
  if [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
    dir=${XDG_RUNTIME_DIR%/}
  elif [[ -n ${HOME:-} ]]; then
    dir=${HOME%/}/.cache/fast_autocomplete
  else
    dir=/tmp
  fi
  print -- "${dir}/fast_autocomplete_${uid}.sock"
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
  local value=$1
  [[ -z $value ]] && return
  [[ -z $_FA_BIN ]] && return
  "$_FA_BIN" --record "$value" &>/dev/null &!
}

# --------------------------------------------------------------------------- #
#  Completion function                                                          #
# --------------------------------------------------------------------------- #

# Last completions returned by the daemon; reused on exit code 2 (unchanged).
typeset -ga _FA_LAST_COMPLETIONS

_fast_autocomplete() {
  [[ -z $_FA_BIN ]] && return 1

  local sock
  sock=$(_fa_socket_path)
  if [[ ! -S $sock ]]; then
    _fa_ensure_daemon || return 1
    [[ -S $sock ]] || return 1
  fi

  local -a _fa_exp
  _fa_exp=( ${(f)"$(_fa_alias_expand "$BUFFER" "$CURSOR")"} )
  local _fa_cursor=${_fa_exp[1]} _fa_buffer=${_fa_exp[2]}

  local payload
  payload=$(printf 'BUFFER=%s\nCURSOR=%d\nCWD=%s\nSESSION=%d\n\n' \
    "$_fa_buffer" "$_fa_cursor" "$PWD" "$$")

  local -a completions
  completions=( ${(f)"$("$_FA_BIN" --complete <<< "$payload")"} )
  local rc=$?

  case $rc in
    2)
      (( ${#_FA_LAST_COMPLETIONS} == 0 )) && return 1
      completions=( "${_FA_LAST_COMPLETIONS[@]}" )
      ;;
    0)
      _FA_LAST_COMPLETIONS=( "${completions[@]}" )
      ;;
    *)
      return 1
      ;;
  esac

  if [[ $_FA_REVERSE_COMPLETE == 1 ]]; then
    completions=( "${(Oa)completions[@]}" )
  fi
  compadd -V fast_autocomplete -Q -U -- "${completions[@]}"
  compstate[insert]='menu:1'
}

# --------------------------------------------------------------------------- #
#  As-you-type display (zle -M area below the prompt)                          #
# --------------------------------------------------------------------------- #

typeset -g _FA_PREV_BUFFER_DISPLAY=''
typeset -g _FA_LAST_DISPLAY=''

_fa_update_below() {
  # Skip when there are pending keystrokes — avoids blocking mid-rapid-type
  (( PENDING )) && return
  # Skip when nothing has changed (cursor moves, redraws, etc.)
  [[ $BUFFER == $_FA_PREV_BUFFER_DISPLAY ]] && return
  _FA_PREV_BUFFER_DISPLAY=$BUFFER

  if [[ -z ${BUFFER// } ]]; then
    zle -M ''
    _FA_LAST_DISPLAY=''
    return
  fi

  [[ -z $_FA_BIN ]] && return

  local sock
  sock=$(_fa_socket_path)
  if [[ ! -S $sock ]]; then
    zle -M ''
    return
  fi

  local -a _fa_exp
  _fa_exp=( ${(f)"$(_fa_alias_expand "$BUFFER" "$CURSOR")"} )
  local _fa_cursor=${_fa_exp[1]} _fa_buffer=${_fa_exp[2]}

  local payload
  payload=$(printf 'BUFFER=%s\nCURSOR=%d\nCWD=%s\nSESSION=%d\n\n' \
    "$_fa_buffer" "$_fa_cursor" "$PWD" "$(( $$ + 1000000 ))")

  local display rc
  display=$("$_FA_BIN" --display "$COLUMNS" <<< "$payload")
  rc=$?

  case $rc in
    2)
      # Unchanged — re-render the last display string without re-querying.
      [[ -n $_FA_LAST_DISPLAY ]] && zle -M -- "$_FA_LAST_DISPLAY"
      return
      ;;
    0)
      _FA_LAST_DISPLAY=$display
      zle -M -- "$display"
      ;;
    *)
      _FA_LAST_DISPLAY=''
      zle -M ''
      ;;
  esac
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
  _FA_LAST_DISPLAY=''
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

# Re-apply the binding on every prompt. Other plugins (zsh-autosuggestions,
# oh-my-zsh, etc.) often rebind ^[[Z after we source, so a one-shot bindkey at
# load time gets clobbered.
_fa_bind_shift_tab() {
  bindkey '^[[Z' _fa_reverse_complete_widget
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd _fa_bind_shift_tab
_fa_bind_shift_tab

# --------------------------------------------------------------------------- #
#  Plugin setup                                                                 #
# --------------------------------------------------------------------------- #

# Register _fast_autocomplete as the first completer.
zstyle ':completion:*' completer _fast_autocomplete _complete _files

# Cache the binary path at load time so hot paths don't re-resolve it.
typeset -g _FA_BIN
_FA_BIN=$(_fa_bin_path)

# Kick off the daemon if the socket is absent.
if [[ -n $_FA_BIN && ! -S $(_fa_socket_path) ]]; then
  _fa_launch "$_FA_BIN"
fi
