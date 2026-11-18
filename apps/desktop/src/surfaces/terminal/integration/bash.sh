# Prompt marks for bash (Git Bash, WSL), run by the terminal when the user turns on "Mark
# commands" for that shell: bash starts with this as its --init-file, which first runs what bash
# runs itself (~/.bashrc, or the login files when OC_LOGIN is set), then adds OSC 133 (A prompt,
# B input, C output, D;<exit> done) and the folder as OSC 7. Nothing is written to those files.
if [ -n "$OC_LOGIN" ]; then
  unset OC_LOGIN
  [ -r /etc/profile ] && . /etc/profile
  for __oc_f in ~/.bash_profile ~/.bash_login ~/.profile; do
    [ -r "$__oc_f" ] && { . "$__oc_f"; break; }
  done
  unset __oc_f
else
  [ -r ~/.bashrc ] && . ~/.bashrc
fi
__oc_prompt() {
  local code=$?
  printf '\e]133;D;%s\a\e]7;file://%s%s\a\e]133;A\a' "$code" "$HOSTNAME" "$PWD"
  case "$PS1" in *'133;B'*) ;; *) PS1="$PS1"'\[\e]133;B\a\]' ;; esac
}
PROMPT_COMMAND="__oc_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0=$'\e]133;C\a'"${PS0:-}"
