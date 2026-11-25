//! A repository whose configuration names programs everywhere git looks: under `Policy::Untrusted` none of them runs
//! whatever the panel does, under `Policy::Trusted` they run as with the user's own git, which shows the setup is real.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{Scratch, write};
use term_git::{
    Cancel, CommitOptions, DiffSource, FetchOptions, Git, Policy, Repository, StashOptions, StatusOptions, Unstash,
};
use term_process::Supervisor;

/// The hooks the hostile repository installs, in `.git/hooks` and in a folder its included configuration points at.
const HOOKS: [&str; 9] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "post-checkout",
    "reference-transaction",
    "post-index-change",
    "pre-auto-gc",
    "post-rewrite",
];

/// A program at `path` that leaves `marks/<name>` when it runs, then runs `rest`.
fn program_then(path: &Path, marks: &Path, name: &str, rest: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let script = format!("#!/bin/sh\ntouch '{}'\n{rest}\n", marks.join(name).display());
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A filter or hook that leaves `marks/<name>` and passes its input through, or reads it all.
fn program(path: &Path, marks: &Path, name: &str) {
    let rest = if name.ends_with("clean") || name.ends_with("smudge") || name == "textconv" {
        "cat"
    } else {
        "cat >/dev/null; exit 0"
    };
    program_then(path, marks, name, rest);
}

/// What ran, by the marks left in `marks`.
fn ran(marks: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(marks)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The folder `marks/` for what runs, and a git whose PATH starts with a stand-in `ssh` that fails at once, so no
/// test reaches the network.
struct Hostile {
    scratch: Scratch,
    marks: PathBuf,
    git: Git,
}

impl Hostile {
    fn new(name: &str) -> Hostile {
        let scratch = Scratch::new(name);
        let marks = scratch.path("marks");
        std::fs::create_dir_all(&marks).unwrap();
        let bin = scratch.path("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("ssh"), "#!/bin/sh\nexit 255\n").unwrap();
        std::fs::set_permissions(bin.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = scratch.environment();
        for (key, value) in &mut env {
            if key == "PATH" {
                *value = format!("{}:{value}", bin.display());
            }
        }
        let git = Git::new(&Supervisor::new(), env);
        Hostile { scratch, marks, git }
    }

    /// A repository with a first commit, then a configuration that runs a program for every filter, driver, helper
    /// and hook, some of it only through an `includeIf` file inside the working tree.
    fn repository(&self) -> PathBuf {
        let repository = self.scratch.repository("r", &[("a.txt", "a\n"), ("b.dat", "b\n")]);
        let root = repository.root().to_path_buf();
        let tools = self.scratch.path("tools");
        for name in ["clean", "smudge", "textconv", "other-clean"] {
            program(&tools.join(name), &self.marks, name);
        }
        for name in ["fsmonitor", "ssh", "pager", "askpass"] {
            program_then(&tools.join(name), &self.marks, name, "exit 1");
        }
        for hook in HOOKS {
            program(&root.join(".git/hooks").join(hook), &self.marks, hook);
            program(
                &root.join("planted").join(hook),
                &self.marks,
                &format!("planted-{hook}"),
            );
        }
        let tool = |name: &str| tools.join(name).display().to_string();
        let config = format!(
            "[filter \"evil\"]\n\tclean = {clean}\n\tsmudge = {smudge}\n\trequired = true\n\
             [diff \"evil\"]\n\ttextconv = {textconv}\n\
             [core]\n\tfsmonitor = {fsmonitor}\n\tsshCommand = {ssh}\n\tpager = {pager}\n\taskPass = {askpass}\n\
             [remote \"origin\"]\n\turl = ssh://example.invalid/r.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
             [includeIf \"gitdir:{root}/\"]\n\tpath = ../extra.conf\n",
            clean = tool("clean"),
            smudge = tool("smudge"),
            textconv = tool("textconv"),
            fsmonitor = tool("fsmonitor"),
            ssh = tool("ssh"),
            pager = tool("pager"),
            askpass = tool("askpass"),
            root = root.display(),
        );
        let mut text = std::fs::read_to_string(root.join(".git/config")).unwrap();
        text.push_str(&config);
        std::fs::write(root.join(".git/config"), text).unwrap();
        write(
            &root,
            "extra.conf",
            &format!(
                "[filter \"other\"]\n\tclean = {}\n\trequired = true\n[core]\n\thooksPath = planted\n",
                tool("other-clean")
            ),
        );
        write(
            &root,
            ".gitattributes",
            "*.txt filter=evil diff=evil\n*.dat filter=other\n",
        );
        root
    }

    fn open(&self, root: &Path, policy: Policy) -> Repository {
        self.git.discover(root, policy).unwrap()
    }
}

/// Everything the Git panel does, on `repository`.
fn exercise(hostile: &Hostile, repository: &Repository) {
    let root = repository.root().to_path_buf();
    write(&root, "a.txt", "changed\n");
    write(&root, "b.dat", "changed\n");
    repository.status(StatusOptions::default()).unwrap();
    repository.diff(DiffSource::Unstaged, &["a.txt", "b.dat"]).unwrap();
    repository
        .stage(&["a.txt", "b.dat", ".gitattributes", "extra.conf"])
        .unwrap();
    repository.diff(DiffSource::Staged, &["a.txt"]).unwrap();
    repository.commit("change", CommitOptions::default()).unwrap();
    write(&root, "a.txt", "stashed\n");
    repository.stash(&StashOptions::default()).unwrap();
    let stash = repository.stashes().unwrap().remove(0);
    repository.stash_changes(&stash.hash).unwrap();
    repository.apply_stash(&stash, Unstash::Pop).unwrap();
    let added = repository
        .add_worktree(&hostile.scratch.path("wt"), "topic", None)
        .unwrap();
    assert_eq!(added.policy(), repository.policy());
    added.status(StatusOptions::default()).unwrap();
    repository.blame("HEAD", Path::new("a.txt")).unwrap();
    repository.log(10).unwrap();
    let history = repository.history(10, None).unwrap();
    repository.commit_changes(&history[0].hash).unwrap();
    repository.diff_commit(&history[0].hash, Path::new("a.txt")).unwrap();
    repository.switch_to_new_branch("other", None).unwrap();
    let fetched = repository.fetch(&FetchOptions::default(), &mut |_| {}, &Cancel::new());
    assert!(fetched.is_err(), "the stand-in ssh fails");
}

#[test]
fn an_untrusted_repository_runs_none_of_its_programs_and_a_trusted_one_does() {
    let hostile = Hostile::new("untrusted-all");
    let root = hostile.repository();

    let untrusted = hostile.open(&root, Policy::Untrusted);
    assert_eq!(untrusted.policy(), Policy::Untrusted);
    exercise(&hostile, &untrusted);
    assert_eq!(
        ran(&hostile.marks),
        Vec::<String>::new(),
        "programs the repository names ran"
    );
    let log = hostile.scratch.raw(&root, &["log", "--format=%s", "-1", "main"]);
    assert_eq!(log.trim(), "change", "the commit went through without its hooks");

    let trusted = hostile.open(&root, Policy::Trusted);
    write(&root, "a.txt", "trusted\n");
    write(&root, "b.dat", "trusted\n");
    trusted.status(StatusOptions::default()).unwrap();
    trusted.stage(&["a.txt", "b.dat"]).unwrap();
    trusted.commit("trusted", CommitOptions::default()).unwrap();
    let _ = trusted.fetch(&FetchOptions::default(), &mut |_| {}, &Cancel::new());
    let marks = ran(&hostile.marks);
    for expected in [
        "planted-pre-commit",
        "planted-commit-msg",
        "clean",
        "other-clean",
        "fsmonitor",
        "ssh",
    ] {
        assert!(marks.iter().any(|m| m == expected), "{expected} did not run: {marks:?}");
    }
}

/// Points the working tree at `root`'s `.git` file to a git folder `.x` of its own, whose configuration has a clean
/// filter and a textconv for every file.
fn redirect(hostile: &Hostile, root: &Path) {
    let planted = hostile.scratch.path("planted");
    std::fs::create_dir_all(&planted).unwrap();
    hostile.scratch.raw(&planted, &["init", "--quiet"]);
    std::fs::rename(planted.join(".git"), root.join(".x")).unwrap();
    let x = root.join(".x");
    let x = x.to_str().unwrap();
    let tools = hostile.scratch.path("tools");
    program(&tools.join("clean"), &hostile.marks, "clean");
    program(&tools.join("textconv"), &hostile.marks, "textconv");
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.org")] {
        hostile.scratch.raw(root, &["--git-dir", x, "config", key, value]);
    }
    for (key, name) in [("filter.evil.clean", "clean"), ("diff.evil.textconv", "textconv")] {
        let value = tools.join(name).display().to_string();
        hostile.scratch.raw(root, &["--git-dir", x, "config", key, &value]);
    }
    write(root, ".gitattributes", "* filter=evil diff=evil\n");
    write(root, ".git", "gitdir: .x\n");
}

#[test]
fn a_git_file_pointing_elsewhere_is_followed_and_neutralised_too() {
    let hostile = Hostile::new("untrusted-redirect");
    let repository = hostile.scratch.repository("r", &[("a.txt", "a\n")]);
    let copy = repository
        .add_worktree(&hostile.scratch.path("wt"), "topic", None)
        .unwrap();
    let root = copy.root().to_path_buf();
    redirect(&hostile, &root);
    write(&root, "a.txt", "changed\n");

    let untrusted = hostile.open(&root, Policy::Untrusted);
    assert_eq!(untrusted.git_dir(), root.join(".x"));
    untrusted.status(StatusOptions::default()).unwrap();
    untrusted.stage_all().unwrap();
    untrusted.commit("untrusted", CommitOptions::default()).unwrap();
    assert_eq!(ran(&hostile.marks), Vec::<String>::new());

    write(&root, "a.txt", "again\n");
    let trusted = hostile.open(&root, Policy::Trusted);
    trusted.stage_all().unwrap();
    assert_eq!(ran(&hostile.marks), ["clean"]);
}

#[test]
fn a_hooks_folder_planted_after_the_repository_was_opened_runs_neither() {
    let hostile = Hostile::new("untrusted-planted");
    let repository = hostile.scratch.repository("r", &[("a.txt", "a\n")]);
    let root = repository.root().to_path_buf();
    let untrusted = hostile.open(&root, Policy::Untrusted);
    program(&root.join("later/pre-commit"), &hostile.marks, "pre-commit");
    hostile.scratch.raw(&root, &["config", "core.hooksPath", "later"]);
    write(&root, "a.txt", "b\n");
    untrusted.stage(&["a.txt"]).unwrap();
    untrusted.commit("untrusted", CommitOptions::default()).unwrap();
    assert_eq!(ran(&hostile.marks), Vec::<String>::new());
}

#[test]
fn a_bare_repository_inside_an_untrusted_folder_is_not_found_by_walking_into_it() {
    let hostile = Hostile::new("untrusted-bare");
    let outer = hostile.scratch.path("outer");
    std::fs::create_dir_all(&outer).unwrap();
    hostile.scratch.raw(&outer, &["init", "--quiet", "--bare", "inner.git"]);
    let inner = outer.join("inner.git");
    let fsmonitor = hostile.scratch.path("tools/fsmonitor");
    program_then(&fsmonitor, &hostile.marks, "fsmonitor", "exit 1");
    hostile
        .scratch
        .raw(&inner, &["config", "core.fsmonitor", fsmonitor.to_str().unwrap()]);
    assert!(hostile.git.discover(&inner, Policy::Untrusted).is_err());
    assert_eq!(ran(&hostile.marks), Vec::<String>::new());
}
