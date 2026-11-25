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

    function __ide_prompt --on-event fish_prompt
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
