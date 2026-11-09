# Source this file from ~/.zshrc.
# Capture the project path when sourced, not when the function is called.
typeset -g _ls_interactive_project=${${(%):-%x}:A:h}

ls-interactive() {
    local executable file destination status_code
    if (( $+commands[ls-interactive] )); then
        executable=$commands[ls-interactive]
    else
        executable="$_ls_interactive_project/target/release/ls-interactive"
    fi

    file=$(mktemp) || return
    LS_INTERACTIVE_CWD_FILE="$file" "$executable" "$@"
    status_code=$?
    destination=$(<"$file")
    rm -f -- "$file"
    if (( status_code == 0 )) && [[ -n "$destination" ]]; then
        cd -- "$destination" || return
    fi
    return "$status_code"
}
