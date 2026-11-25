//! Remotes and the operations that reach them: fetch, pull and push, with progress and a way to cancel.

use crate::error::{Error, Result};
use crate::repo::Repository;
use crate::run::{Cancel, Output};
use crate::said::{Said, overwritten_paths};

/// A remote with the URLs git uses for it, `insteadOf` rewrites applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Its name, such as `origin`.
    pub name: String,
    /// Where fetches go.
    pub fetch_url: Option<String>,
    /// Where pushes go; the fetch URL unless a push URL is set.
    pub push_urls: Vec<String>,
}

/// How far a long operation has come, from a progress line git writes to standard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// What git does, such as `Receiving objects`.
    pub phase: String,
    /// The share done, when git gives one.
    pub percent: Option<u8>,
    /// How many items are done.
    pub(crate) done: Option<u64>,
    /// How many items there are, when known.
    pub(crate) total: Option<u64>,
    /// Whether the server reports it rather than the local git.
    pub(crate) remote: bool,
}

/// What to fetch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchOptions {
    /// The remote; `None` for the current branch's, else `origin`.
    pub remote: Option<String>,
    /// Fetch every remote; `remote` is then ignored.
    pub all: bool,
    /// Drop remote-tracking branches whose branch is gone on the remote.
    pub prune: bool,
}

/// How a pull joins the fetched commits with local ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullMode {
    /// Only when the branch has nothing of its own; otherwise `Error::NotFastForward`.
    #[default]
    FastForwardOnly,
    /// Replays the branch's own commits on top.
    Rebase,
    /// Joins both in a merge commit, or fast-forwards when the branch has nothing of its own.
    Merge,
}

/// What to pull.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullOptions {
    /// How to join.
    pub mode: PullMode,
    /// The remote; `None` for the upstream.
    pub remote: Option<String>,
    /// The remote's branch; only with `remote`.
    pub branch: Option<String>,
}

/// What to push.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushOptions {
    /// The remote; `None` for the branch's push remote.
    pub remote: Option<String>,
    /// What to push, such as `HEAD` or `main:main`; only with `remote`.
    pub refspec: Option<String>,
    /// Make the remote branch the upstream of the local one, for a branch's first push.
    pub set_upstream: bool,
}

/// What a push did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushOutcome {
    /// Every ref the push touched.
    pub refs: Vec<PushedRef>,
    /// What the server said, such as a link to open a merge request.
    pub(crate) messages: Vec<String>,
}

/// One ref of a push, from `git push --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedRef {
    /// What happened to it.
    pub flag: PushFlag,
    /// The local ref.
    pub(crate) from: String,
    /// The remote ref.
    pub to: String,
    /// git's summary, such as `[new branch]` or `[rejected] (fetch first)`.
    pub(crate) summary: String,
}

/// What a push did with a ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushFlag {
    /// Fast-forwarded.
    FastForward,
    /// Forced.
    Forced,
    /// Deleted.
    Deleted,
    /// Created.
    New,
    /// Rejected, or the remote failed to update it.
    Rejected,
    /// Already up to date.
    UpToDate,
}

impl Repository {
    /// The remotes with their URLs.
    pub fn remotes(&self) -> Result<Vec<Remote>> {
        let output = self.git().ok(self.call(["remote", "-v"]))?;
        Ok(parse_remotes(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Fetches, reporting progress to `progress` until done or `cancel` is requested.
    pub fn fetch(&self, options: &FetchOptions, progress: &mut dyn FnMut(&Progress), cancel: &Cancel) -> Result<()> {
        let mut args = vec!["fetch", "--progress"];
        if options.prune {
            args.push("--prune");
        }
        if options.all {
            args.push("--all");
        } else if let Some(remote) = &options.remote {
            args.push(word(remote)?);
        }
        let mut messages = Vec::new();
        let (output, _) = self.remote_call(&args, progress, &mut messages, cancel)?;
        if output.code == 0 {
            Ok(())
        } else {
            Err(crate::run::failure(&output))
        }
    }

    /// Pulls into the current branch, fast-forward only unless `options` say rebase.
    pub fn pull(&self, options: &PullOptions, progress: &mut dyn FnMut(&Progress), cancel: &Cancel) -> Result<()> {
        let mut args = vec!["pull", "--progress"];
        args.extend(match options.mode {
            PullMode::FastForwardOnly => ["--no-rebase", "--ff-only"],
            PullMode::Rebase => ["--rebase", "--no-autostash"],
            PullMode::Merge => ["--no-rebase", "--ff"],
        });
        if let Some(remote) = &options.remote {
            args.push(word(remote)?);
            if let Some(branch) = &options.branch {
                args.push(word(branch)?);
            }
        }
        let mut messages = Vec::new();
        let (output, failed_hook) = self.remote_call(&args, progress, &mut messages, cancel)?;
        if output.code == 0 {
            return Ok(());
        }
        if let Some(hook) = failed_hook {
            return Err(Error::HookFailed {
                hook,
                output: output.said(),
            });
        }
        let said = output.said();
        if Said::NotFastForward.in_text(&said) {
            return Err(Error::NotFastForward { output: said });
        }
        if options.remote.is_none()
            && !matches!(
                crate::run::failure(&output),
                Error::AuthFailed { .. } | Error::Network { .. }
            )
            && self.lacks_upstream()?
        {
            return Err(Error::NoUpstream { output: said });
        }
        if Said::LocalChanges.in_text(&said) {
            return Err(Error::LocalChanges {
                paths: overwritten_paths(&said),
                output: said,
            });
        }
        let conflicts = self.unmerged_paths()?;
        if !conflicts.is_empty() {
            return Err(Error::Conflicts {
                paths: conflicts,
                output: said,
            });
        }
        Err(crate::run::failure(&output))
    }

    /// Pushes, failing with `Error::PushRejected` when the remote refuses a ref.
    pub fn push(
        &self,
        options: &PushOptions,
        progress: &mut dyn FnMut(&Progress),
        cancel: &Cancel,
    ) -> Result<PushOutcome> {
        self.push_with(options, false, progress, cancel)
    }

    pub(crate) fn push_with(
        &self,
        options: &PushOptions,
        force: bool,
        progress: &mut dyn FnMut(&Progress),
        cancel: &Cancel,
    ) -> Result<PushOutcome> {
        let mut args = vec!["push", "--progress", "--porcelain"];
        if options.set_upstream {
            args.push("--set-upstream");
        }
        if force {
            args.push("--force-with-lease");
        }
        if let Some(remote) = &options.remote {
            args.push(word(remote)?);
            if let Some(refspec) = &options.refspec {
                args.push(word(refspec)?);
            }
        }
        let mut messages = Vec::new();
        let (output, failed_hook) = self.remote_call(&args, progress, &mut messages, cancel)?;
        let refs = parse_push(&String::from_utf8_lossy(&output.stdout));
        if output.code == 0 {
            return Ok(PushOutcome { refs, messages });
        }
        if let Some(hook) = failed_hook {
            return Err(Error::HookFailed {
                hook,
                output: output.said(),
            });
        }
        let said = output.said();
        if refs.iter().any(|r| r.flag == PushFlag::Rejected) {
            return Err(Error::PushRejected { refs, output: said });
        }
        let failure = crate::run::failure(&output);
        let nowhere = options.refspec.is_none()
            && matches!(failure, Error::Failed { .. })
            && (self.lacks_upstream()? || options.remote.is_none() && self.remotes()?.is_empty());
        if nowhere {
            return Err(Error::NoUpstream { output: said });
        }
        Err(failure)
    }

    /// Whether HEAD is on a branch that tracks none.
    fn lacks_upstream(&self) -> Result<bool> {
        Ok(self.current_branch()?.is_some() && self.resolve("@{upstream}")?.is_none())
    }

    /// Runs a command that talks to a remote: no time limit, cancellable, progress parsed, server messages kept.
    fn remote_call(
        &self,
        args: &[&str],
        progress: &mut dyn FnMut(&Progress),
        messages: &mut Vec<String>,
        cancel: &Cancel,
    ) -> Result<(Output, Option<String>)> {
        let mut on_segment = |segment: &str| {
            if let Some(report) = parse_progress(segment) {
                progress(&report);
            } else if let Some(message) = segment.strip_prefix("remote: ") {
                let message = message.trim_end();
                if !message.is_empty() {
                    messages.push(message.to_string());
                }
            }
        };
        let call = self.call(args).unlimited().cancel(cancel).progress(&mut on_segment);
        crate::hooks::run(self.git(), call)
    }
}

/// `name` as an argument that git cannot take for an option.
pub(crate) fn word(name: &str) -> Result<&str> {
    if name.is_empty() || name.starts_with('-') {
        return Err(Error::InvalidName { name: name.to_string() });
    }
    Ok(name)
}

/// The progress in one piece of git's standard error, such as `Receiving objects:  45% (9/20), 1.2 MiB | 3 MiB/s`.
pub(crate) fn parse_progress(segment: &str) -> Option<Progress> {
    let (text, remote) = match segment.strip_prefix("remote: ") {
        Some(rest) => (rest, true),
        None => (segment, false),
    };
    let (phase, rest) = text.split_once(": ")?;
    let rest = rest.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || phase.is_empty() || phase.contains(['/', '\'']) {
        return None;
    }
    let number: u64 = rest[..digits].parse().ok()?;
    let after = &rest[digits..];
    let (percent, done, total) = if after.starts_with('%') {
        let counts = after
            .split_once('(')
            .and_then(|(_, r)| r.split_once(')'))
            .and_then(|(inside, _)| inside.split_once('/'))
            .and_then(|(d, t)| Some((d.trim().parse().ok()?, t.trim().parse().ok()?)));
        (
            u8::try_from(number.min(100)).ok(),
            counts.map(|c| c.0),
            counts.map(|c| c.1),
        )
    } else if after.is_empty() || after.starts_with(',') {
        (None, Some(number), None)
    } else {
        return None;
    };
    Some(Progress {
        phase: phase.to_string(),
        percent,
        done,
        total,
        remote,
    })
}

fn parse_remotes(text: &str) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((url, kind)) = rest.rsplit_once(' ') else {
            continue;
        };
        let index = match remotes.iter().position(|r| r.name == name) {
            Some(index) => index,
            None => {
                remotes.push(Remote {
                    name: name.to_string(),
                    fetch_url: None,
                    push_urls: Vec::new(),
                });
                remotes.len() - 1
            }
        };
        match kind {
            "(fetch)" => remotes[index].fetch_url = Some(url.to_string()),
            "(push)" => remotes[index].push_urls.push(url.to_string()),
            _ => {}
        }
    }
    remotes
}

fn parse_push(text: &str) -> Vec<PushedRef> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let flag = match fields.next()? {
                " " => PushFlag::FastForward,
                "+" => PushFlag::Forced,
                "-" => PushFlag::Deleted,
                "*" => PushFlag::New,
                "!" => PushFlag::Rejected,
                "=" => PushFlag::UpToDate,
                _ => return None,
            };
            let (from, to) = fields.next()?.split_once(':')?;
            let summary = fields.next().unwrap_or_default().to_string();
            Some(PushedRef {
                flag,
                from: from.to_string(),
                to: to.to_string(),
                summary,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_give_phase_share_and_counts() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (9/20), 1.20 MiB | 3.00 MiB/s"),
            Some(Progress {
                phase: "Receiving objects".into(),
                percent: Some(45),
                done: Some(9),
                total: Some(20),
                remote: false
            })
        );
        assert_eq!(
            parse_progress("remote: Enumerating objects: 8, done."),
            Some(Progress {
                phase: "Enumerating objects".into(),
                percent: None,
                done: Some(8),
                total: None,
                remote: true
            })
        );
        assert_eq!(
            parse_progress("Writing objects: 100% (8/8), 586.58 KiB | 22.56 MiB/s, done.")
                .unwrap()
                .percent,
            Some(100)
        );
    }

    #[test]
    fn other_lines_are_not_progress() {
        for line in [
            "Total 8 (delta 0), reused 0 (delta 0)",
            "Delta compression using up to 8 threads",
            "To /tmp/bare.git",
            "remote: Create a merge request for feature:",
            "fatal: unable to access 'https://x/': Could not resolve host: x",
            "hint: Updates were rejected",
        ] {
            assert_eq!(parse_progress(line), None, "{line}");
        }
    }

    #[test]
    fn remotes_collect_their_fetch_and_push_urls() {
        let text = "origin\tgit@github.com:a/b.git (fetch)\norigin\tgit@github.com:a/b.git (push)\n\
                    mirror\thttps://gitlab.example/x.git (fetch)\nmirror\thttps://one/x.git (push)\nmirror\thttps://two/x.git (push)\n";
        let remotes = parse_remotes(text);
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].fetch_url.as_deref(), Some("git@github.com:a/b.git"));
        assert_eq!(remotes[1].push_urls, ["https://one/x.git", "https://two/x.git"]);
    }

    #[test]
    fn porcelain_push_lines_are_read() {
        let text = "To /tmp/bare.git\n*\trefs/heads/main:refs/heads/main\t[new branch]\n\
                    !\trefs/heads/x:refs/heads/x\t[rejected] (fetch first)\nbranch 'main' set up to track 'origin/main'.\nDone\n";
        let refs = parse_push(text);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].flag, PushFlag::New);
        assert_eq!(refs[0].to, "refs/heads/main");
        assert_eq!(refs[1].flag, PushFlag::Rejected);
        assert_eq!(refs[1].summary, "[rejected] (fetch first)");
    }
}
