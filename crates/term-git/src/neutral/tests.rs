use std::ffi::OsString;
use std::path::Path;

use super::*;

/// The settings `environment` gives for a git started in `dir`, as `key=value` lines, and the other variables.
fn settings_in(dir: &Path, users: &[u8]) -> (Vec<String>, Vec<(String, String)>) {
    let home: Vec<(OsString, OsString)> = vec![("HOME".into(), dir.join("home").into_os_string())];
    let env = environment(dir, &home, |_| UserConfig::parse(&[Some(users.to_vec())])).unwrap();
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    };
    let count: usize = get("GIT_CONFIG_COUNT").unwrap().parse().unwrap();
    let settings = (0..count)
        .map(|i| {
            format!(
                "{}={}",
                get(&format!("GIT_CONFIG_KEY_{i}")).unwrap(),
                get(&format!("GIT_CONFIG_VALUE_{i}")).unwrap()
            )
        })
        .collect();
    let others = env
        .iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .filter(|(k, _)| !k.starts_with("GIT_CONFIG_"))
        .collect();
    (settings, others)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_folder_outside_any_repository_gets_the_fixed_settings() {
    let scratch = term_base::testing::scratch("neutral-none");
    let (settings, others) = settings_in(&scratch, b"");
    assert_eq!(settings.len(), FIXED.len());
    assert!(settings.contains(&"core.hookspath=/dev/null".to_string()));
    assert!(settings.contains(&"core.fsmonitor=false".to_string()));
    assert_eq!(others, [("GIT_ATTR_NOSYSTEM".to_string(), "1".to_string())]);
}

#[test]
fn every_key_that_names_a_program_is_reset() {
    let scratch = term_base::testing::scratch("neutral-keys");
    write(
        &scratch.join(".git/config"),
        "[filter \"x=y\"]\n\tclean = evil\n\tsmudge = evil\n\trequired\n[diff \"d\"]\n\ttextconv = evil\n\
         [diff]\n\texternal = evil\n[merge \"m\"]\n\tdriver = evil\n[core]\n\tsshCommand = evil\n\tpager = evil\n\
         \taskPass = evil\n\talternateRefsCommand = evil\n[pager]\n\tlog = evil\n[gpg]\n\tprogram = evil\n\
         [gpg \"ssh\"]\n\tdefaultKeyCommand = evil\n[remote \"origin\"]\n\tuploadpack = evil\n\treceivepack = evil\n\
         [uploadpack]\n\tpackObjectsHook = evil\n[user]\n\tname = kept\n",
    );
    let (settings, _) = settings_in(&scratch, b"");
    let dynamic = &settings[FIXED.len()..];
    assert_eq!(
        dynamic,
        [
            "filter.x=y.clean=",
            "filter.x=y.required=false",
            "filter.x=y.smudge=",
            "diff.d.textconv=cat",
            "diff.external=",
            "merge.m.driver=false",
            "core.sshcommand=ssh",
            "core.pager=",
            "core.askpass=",
            "core.alternaterefscommand=",
            "pager.log=false",
            "gpg.program=",
            "gpg.ssh.defaultkeycommand=",
            "remote.origin.uploadpack=git-upload-pack",
            "remote.origin.receivepack=git-receive-pack",
            "uploadpack.packobjectshook=",
        ]
    );
}

#[test]
fn the_users_own_values_come_back_and_credential_helpers_are_listed_again() {
    let scratch = term_base::testing::scratch("neutral-users");
    write(
        &scratch.join(".git/config"),
        "[credential]\n\thelper = evil\n[filter \"lfs\"]\n\tprocess = evil\n[core]\n\tsshCommand = evil\n",
    );
    let users = b"credential.helper\nosxkeychain\0credential.helper\nstore\0\
filter.lfs.process\ngit-lfs filter-process\0filter.lfs.required\ntrue\0core.sshcommand\nssh -i key\0";
    let (settings, _) = settings_in(&scratch, users);
    assert_eq!(
        &settings[FIXED.len()..],
        [
            "credential.helper=",
            "credential.helper=osxkeychain",
            "credential.helper=store",
            "filter.lfs.process=git-lfs filter-process",
            "filter.lfs.required=true",
            "core.sshcommand=ssh -i key",
        ]
    );
}

#[test]
fn includes_conditional_ones_and_the_worktrees_config_are_followed() {
    let scratch = term_base::testing::scratch("neutral-includes");
    write(
        &scratch.join("main/.git/config"),
        "[include]\n\tpath = more.conf\n[includeIf \"gitdir:/nowhere/\"]\n\tpath = ~/conditional.conf\n",
    );
    write(&scratch.join("main/.git/more.conf"), "[diff \"a\"]\n\tcommand = evil\n");
    write(
        &scratch.join("w/home/conditional.conf"),
        "[filter \"b\"]\n\tclean = evil\n",
    );
    write(&scratch.join("main/.git/worktrees/w/commondir"), "../..\n");
    write(
        &scratch.join("main/.git/worktrees/w/config.worktree"),
        "[core]\n\tpager = evil\n",
    );
    write(
        &scratch.join("w/.git"),
        &format!("gitdir: {}\n", scratch.join("main/.git/worktrees/w").display()),
    );
    let (settings, _) = settings_in(&scratch.join("w"), b"");
    let dynamic = &settings[FIXED.len()..];
    assert_eq!(
        dynamic,
        [
            "diff.a.command=",
            "filter.b.clean=",
            "filter.b.required=false",
            "core.pager="
        ]
    );
}

#[test]
fn submodules_are_read_wherever_git_keeps_them() {
    let scratch = term_base::testing::scratch("neutral-submodules");
    write(&scratch.join(".git/config"), "");
    write(&scratch.join(".git/modules/lib/a/HEAD"), "ref: refs/heads/main\n");
    write(
        &scratch.join(".git/modules/lib/a/config"),
        "[filter \"s\"]\n\tclean = evil\n",
    );
    write(
        &scratch.join(".gitmodules"),
        "[submodule \"old\"]\n\tpath = vendor/old\n",
    );
    write(
        &scratch.join("vendor/old/.git/config"),
        "[diff \"o\"]\n\ttextconv = evil\n",
    );
    let (settings, _) = settings_in(&scratch, b"");
    let dynamic = &settings[FIXED.len()..];
    assert!(dynamic.contains(&"filter.s.clean=".to_string()), "{dynamic:?}");
    assert!(dynamic.contains(&"diff.o.textconv=cat".to_string()), "{dynamic:?}");
}

#[test]
fn a_proxy_command_is_switched_off_through_the_environment() {
    let scratch = term_base::testing::scratch("neutral-proxy");
    write(&scratch.join(".git/config"), "[core]\n\tgitProxy = evil\n");
    let (_, others) = settings_in(&scratch, b"");
    assert!(others.contains(&("GIT_PROXY_COMMAND".to_string(), String::new())));
}

#[test]
fn a_configuration_that_cannot_be_read_stops_git() {
    let scratch = term_base::testing::scratch("neutral-damaged");
    write(&scratch.join(".git/config"), "[core\n\tfsmonitor = evil\n");
    let result = environment(&scratch, &[], |_| UserConfig::default());
    assert!(matches!(result, Err(Error::UncheckedConfig { .. })), "{result:?}");
    write(&scratch.join(".git/config"), "[include]\n\tpath = config\n");
    let result = environment(&scratch, &[], |_| UserConfig::default());
    assert!(
        matches!(result, Err(Error::UncheckedConfig { .. })),
        "an include of itself: {result:?}"
    );
}

#[test]
fn the_user_is_only_asked_when_a_reset_needs_their_values() {
    let scratch = term_base::testing::scratch("neutral-lazy");
    write(&scratch.join(".git/config"), "[core]\n\tpager = evil\n");
    environment(&scratch, &[], |_| panic!("not needed")).unwrap();
}

#[test]
fn a_named_pipe_or_a_folder_for_a_configuration_stops_git_without_waiting() {
    let scratch = term_base::testing::scratch("neutral-fifo");
    std::fs::create_dir_all(scratch.join(".git")).unwrap();
    let fifo = scratch.join(".git/config");
    let made = std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let result = environment(&scratch, &[], |_| UserConfig::default());
    assert!(matches!(result, Err(Error::UncheckedConfig { .. })), "{result:?}");
    std::fs::remove_file(&fifo).unwrap();
    std::fs::create_dir_all(&fifo).unwrap();
    let result = environment(&scratch, &[], |_| UserConfig::default());
    assert!(matches!(result, Err(Error::UncheckedConfig { .. })), "{result:?}");
}

#[test]
fn a_configuration_above_its_bound_stops_git() {
    let scratch = term_base::testing::scratch("neutral-large");
    write(&scratch.join(".git/config"), "");
    let file = std::fs::File::create(scratch.join(".git/config")).unwrap();
    file.set_len(MAX_FILE + 1).unwrap();
    let result = environment(&scratch, &[], |_| UserConfig::default());
    assert!(matches!(result, Err(Error::UncheckedConfig { .. })), "{result:?}");
}

#[test]
fn a_configuration_the_system_refuses_stops_git() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = term_base::testing::scratch("neutral-forbidden");
    let config = scratch.join(".git/config");
    write(&config, "[core]\n\tpager = evil\n");
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = environment(&scratch, &[], |_| UserConfig::default());
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
}
