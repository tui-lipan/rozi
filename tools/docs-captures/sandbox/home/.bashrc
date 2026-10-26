PS1=$'\\[\e[34m\\]\\w\\[\e[0m\\] \\[\e[35m\\]❯\\[\e[0m\\] '
HISTFILE=/dev/null
# The system bashrc titles each pane user@host:dir; name only the directory.
PROMPT_COMMAND=('printf "\033]0;%s\007" "${PWD/#$HOME/\~}"')
