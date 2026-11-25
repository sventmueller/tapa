# bash reads this in place of ~/.bashrc (--init-file). A login shell would not read an init file at all, so this one
# reads the user's startup files the way a login shell does, then marks prompts and commands (OSC 133) and reports
# the working directory (OSC 7). It runs in bash 3.2, the one macOS ships.

# Before the user's files, so that their own ssh function or alias replaces this one; `command ssh` bypasses it.
if [ -n "${IDE_SSH_DIR:-}" ]; then
  __ide_ssh_dir=$IDE_SSH_DIR
  unset IDE_SSH_DIR
  ssh() { /bin/sh "$__ide_ssh_dir/ssh.sh" "$__ide_ssh_dir/remote" "$@"; }
fi

if [ -r /etc/profile ]; then . /etc/profile; fi
if [ -r "$HOME/.bash_profile" ]; then
  . "$HOME/.bash_profile"
elif [ -r "$HOME/.bash_login" ]; then
  . "$HOME/.bash_login"
elif [ -r "$HOME/.profile" ]; then
  . "$HOME/.profile"
fi

__ide_urlencode() {
  local LC_ALL=C text="$1" out="" c hex i
  case "$text" in
    *[!a-zA-Z0-9/._~-]*) ;;
    *) __ide_reply="$text"; return ;;
  esac
  for (( i = 0; i < ${#text}; i++ )); do
    c="${text:i:1}"
    case "$c" in
      [a-zA-Z0-9/._~-]) out+="$c" ;;
      *) printf -v hex '%%%02X' "'$c"; out+="$hex" ;;
    esac
  done
  __ide_reply="$out"
}

__ide_prompt_start() {
  local ret=$?
  if [ -n "$__ide_running" ]; then
    printf '\033]133;D;%s\007' "$ret"
  fi
  __ide_running=
  __ide_in_prompt=1
  return $ret
}

# What the terminal's suggestions and colours need to know, sent when it changes (OSC 633 P with Tapa's keys): whether
# ble.sh draws suggestions of its own, the PATH, and the names bash runs without a program in the PATH.
__ide_report() {
  local LC_ALL=C own=0 names key=TapaNames part
  if [ -n "${BLE_VERSION:-}" ]; then own=1; fi
  if [ "$own" != "$__ide_sent_own" ]; then
    printf '\033]633;P;TapaOwnSuggestions=%s\007' "$own"
    __ide_sent_own=$own
  fi
  if [ "$PATH" != "$__ide_sent_path" ]; then
    part=${PATH//\\/\\\\}
    printf '\033]633;P;TapaPath=%s\007' "${part//;/\\x3b}"
    __ide_sent_path=$PATH
  fi
  names=$(compgen -a -b -k -A function)
  names=${names//$'\n'/ }
  if [ "$names" = "$__ide_sent_names" ]; then return; fi
  __ide_sent_names=$names
  names=${names//\\/\\\\}
  names=${names//;/\\x3b}
  # A sequence longer than 4 KiB is dropped by the terminal, so the list goes in parts cut at spaces.
  while [ ${#names} -gt 3000 ]; do
    part=${names:0:3000}
    part=${part% *}
    printf '\033]633;P;%s=%s\007' "$key" "$part"
    names=${names:${#part}+1}
    key=TapaMoreNames
  done
  printf '\033]633;P;%s=%s\007' "$key" "$names"
}

__ide_prompt_end() {
  local ret=$?
  __ide_report
  __ide_urlencode "$PWD"
  printf '\033]7;file://%s%s\007\033]133;A\007' "$HOSTNAME" "$__ide_reply"
  # Prompt themes rebuild PS1 in PROMPT_COMMAND, which runs before this.
  case "$PS1" in
    *'\[\033]133;B\007\]'*) ;;
    *) PS1="$PS1"'\[\033]133;B\007\]' ;;
  esac
  __ide_in_prompt=
  return $ret
}

__ide_preexec() {
  if [ -n "${COMP_LINE:-}" ] || [ -n "$__ide_in_prompt" ] || [ -n "$__ide_running" ]; then
    return 0
  fi
  case "$BASH_COMMAND" in
    __ide_prompt_start*) return 0 ;;
  esac
  __ide_running=1
  local line
  line=$(HISTTIMEFORMAT= builtin history 1)
  line="${line#*[0-9]  }"
  __ide_urlencode "$line"
  printf '\033]133;C;cmdline_url=%s\007' "$__ide_reply"
  return 0
}

__ide_running=
__ide_in_prompt=
__ide_sent_own=
__ide_sent_path=
__ide_sent_names=

if [ -n "${bash_preexec_imported:-}" ] || declare -F __bp_install >/dev/null 2>&1; then
  # bash-preexec owns the DEBUG trap and PROMPT_COMMAND; it calls these hooks with $? already set.
  __ide_bp_precmd() { __ide_prompt_start; __ide_prompt_end; }
  __ide_bp_preexec() {
    __ide_running=1
    __ide_urlencode "$1"
    printf '\033]133;C;cmdline_url=%s\007' "$__ide_reply"
  }
  precmd_functions+=(__ide_bp_precmd)
  preexec_functions+=(__ide_bp_preexec)
else
  __ide_debug_previous=
  __ide_trap=$(trap -p DEBUG)
  if [ -n "$__ide_trap" ]; then
    # `trap -p` prints `trap -- 'command' DEBUG`; letting the shell parse it unquotes the command.
    eval "__ide_trap=($__ide_trap)"
    __ide_debug_previous="${__ide_trap[2]}"
  fi
  unset __ide_trap
  trap '__ide_preexec; eval "$__ide_debug_previous"' DEBUG
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__ide_prompt_start "${PROMPT_COMMAND[@]}" __ide_prompt_end)
  else
    PROMPT_COMMAND=$'__ide_prompt_start\n'"${PROMPT_COMMAND:-}"$'\n__ide_prompt_end'
  fi
fi
