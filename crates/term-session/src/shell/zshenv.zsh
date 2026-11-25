# zsh reads this through ZDOTDIR, which the terminal points here. Giving ZDOTDIR back first lets zsh read the
# user's .zprofile, .zshrc and .zlogin from where they are, as if this file did not exist.
if [[ -n "${IDE_ZSH_ZDOTDIR+X}" ]]; then
  builtin export ZDOTDIR="$IDE_ZSH_ZDOTDIR"
  builtin unset IDE_ZSH_ZDOTDIR
else
  builtin unset ZDOTDIR
fi

{
  builtin typeset _ide_file="${ZDOTDIR-$HOME}/.zshenv"
  [[ ! -r "$_ide_file" ]] || builtin source -- "$_ide_file"
} always {
  if [[ -o interactive ]]; then
    _ide_file="${${(%):-%x}:A:h}/integration.zsh"
    [[ ! -r "$_ide_file" ]] || builtin source -- "$_ide_file"
  fi
  builtin unset _ide_file
}
