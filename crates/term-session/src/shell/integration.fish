# fish finds this through XDG_DATA_DIRS, which the terminal extended by one folder; it is put back first so that
# programs started from the shell see the user's value. Then prompts and commands are marked (OSC 133) and the
# working directory is reported (OSC 7).
if set -q IDE_FISH_XDG_DATA_DIRS
    set -gx XDG_DATA_DIRS "$IDE_FISH_XDG_DATA_DIRS"
    set -e IDE_FISH_XDG_DATA_DIRS
else
    set -e XDG_DATA_DIRS
end

if status is-interactive
    function __ide_report_pwd --on-variable PWD
        printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
    end

    function __ide_preexec --on-event fish_preexec
        printf '\e]133;C;cmdline_url=%s\a' (string escape --style=url -- $argv[1])
        set -g __ide_running 1
    end

    function __ide_postexec --on-event fish_postexec
        set -l ret $status
        if set -q __ide_running
            printf '\e]133;D;%s\a' $ret
            set -e __ide_running
        end
    end

    # What the terminal's suggestions and colours need to know, sent when it changes (OSC 633 P with Tapa's keys):
    # whether fish draws its own suggestions, the PATH, and the names fish runs without a program in the PATH.
    function __ide_report
        set -l own 1
        if set -q fish_autosuggestion_enabled; and test "$fish_autosuggestion_enabled" = 0
            set own 0
        end
        if test "$own" != "$__ide_sent_own"
            printf '\e]633;P;TapaOwnSuggestions=%s\a' $own
            set -g __ide_sent_own $own
        end
        set -l path (string join : -- $PATH)
        if test "$path" != "$__ide_sent_path"
            printf '\e]633;P;TapaPath=%s\a' (string replace -a ';' '\\x3b' -- (string replace -a '\\' '\\\\' -- $path))
            set -g __ide_sent_path $path
        end
        set -l names (string join ' ' -- (functions -n) (builtin -n) (abbr --list 2>/dev/null))
        test "$names" = "$__ide_sent_names"; and return
        set -g __ide_sent_names $names
        set names (string replace -a ';' '\\x3b' -- (string replace -a '\\' '\\\\' -- $names))
        set -l key TapaNames
        # A sequence longer than 4 KiB is dropped by the terminal, so the list goes in parts cut at spaces.
        while test (string length -- $names) -gt 3000
            set -l part (string replace -r ' [^ ]*$' '' -- (string sub -l 3000 -- $names))
            printf '\e]633;P;%s=%s\a' $key $part
            set names (string sub -s (math (string length -- $part) + 2) -- $names)
            set key TapaMoreNames
        end
        printf '\e]633;P;%s=%s\a' $key $names
    end

    function __ide_prompt --on-event fish_prompt
        __ide_report
        printf '\e]133;A\a'
        # config.fish runs after this file and may define the prompt, so it is wrapped only at the first prompt.
        if not set -q __ide_prompt_wrapped; and functions -q fish_prompt
            set -g __ide_prompt_wrapped 1
            functions -c fish_prompt __ide_user_prompt
            function fish_prompt
                __ide_user_prompt
                printf '\e]133;B\a'
            end
        end
    end

    # config.fish runs later, so a function of the user's own replaces this one; `command ssh` bypasses it.
    if set -q IDE_SSH_DIR
        set -g __ide_ssh_dir $IDE_SSH_DIR
        set -e IDE_SSH_DIR
        function ssh --wraps ssh
            /bin/sh $__ide_ssh_dir/ssh.sh $__ide_ssh_dir/remote $argv
        end
    end

    __ide_report_pwd
end
