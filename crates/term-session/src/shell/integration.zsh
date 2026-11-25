# Marks prompts and commands (OSC 133) and reports the working directory (OSC 7) for the terminal.
# The hooks join at the first prompt, after the user's startup files, so they run after a theme's own.

_ide_urlencode() {
  builtin emulate -L zsh -o extended_glob
  builtin local LC_ALL=C text=$1 out= c hex
  builtin local -i i
  if [[ $text == [[:alnum:]/._~-]# ]]; then
    REPLY=$text
    return
  fi
  for (( i = 1; i <= ${#text}; i++ )); do
    c=$text[i]
    if [[ $c == [[:alnum:]/._~-] ]]; then
      out+=$c
    else
      builtin printf -v hex '%%%02X' "'$c"
      out+=$hex
    fi
  done
  REPLY=$out
}

_ide_precmd() {
  builtin local ret=$?
  builtin emulate -L zsh
  if [[ -n $_ide_running ]]; then
    builtin printf '\e]133;D;%s\a' $ret
    _ide_running=
  fi
  builtin local REPLY
  _ide_urlencode "$PWD"
  builtin printf '\e]7;file://%s%s\a\e]133;A\a' "$HOST" "$REPLY"
  # Themes rebuild PS1 in their own precmd, which runs before this one.
  [[ $PS1 == *$'\e]133;B\a'* ]] || PS1+=$'%{\e]133;B\a%}'
}

_ide_preexec() {
  builtin emulate -L zsh
  builtin local REPLY
  _ide_urlencode "$1"
  builtin printf '\e]133;C;cmdline_url=%s\a' "$REPLY"
  _ide_running=1
}

_ide_first_prompt() {
  builtin emulate -L zsh
  builtin typeset -ga precmd_functions preexec_functions
  precmd_functions=(${precmd_functions:#_ide_first_prompt} _ide_precmd)
  preexec_functions+=(_ide_preexec)
  _ide_precmd
}

builtin typeset -ga precmd_functions
builtin typeset -g _ide_running=
precmd_functions+=(_ide_first_prompt)

# `command ssh` bypasses this, and a function or alias of the user's own, defined later, replaces it.
if [[ -n ${IDE_SSH_DIR-} ]]; then
  builtin typeset -g _ide_ssh_dir=$IDE_SSH_DIR
  builtin unset IDE_SSH_DIR
  ssh() { /bin/sh "$_ide_ssh_dir/ssh.sh" "$_ide_ssh_dir/remote" "$@" }
fi
