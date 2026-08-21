//! Network operations, run through the `git` binary. See [`super::cli`] for why.

use crate::error::Result;
use crate::git::cli;
use crate::job::{Cancel, Progress};
use std::path::Path;

/// What a push should be allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushMode {
    Normal,
    /// `--force-with-lease`, never a bare `--force`.
    ///
    /// A plain force-push discards whatever arrived on the remote since you
    /// last looked, including someone else's work. `--force-with-lease` refuses
    /// unless the remote is where you last saw it, which is what people
    /// actually mean when they say they want to force-push.
    ForceWithLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    Merge,
    Rebase,
    /// Refuse unless the merge is a fast-forward.
    FastForwardOnly,
}

pub fn fetch(
    workdir: &Path,
    remote: &str,
    prune: bool,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let mut args = vec!["fetch", "--progress", remote, "--tags"];
    if prune {
        // Without pruning, branches deleted on the remote linger in the
        // sidebar indefinitely, which is how stale ref lists happen.
        args.push("--prune");
    }
    let output = cli::run(workdir, &args, cancel, on_progress)?;
    Ok(summarize(&output.stderr, "Already up to date"))
}

pub fn fetch_all(
    workdir: &Path,
    prune: bool,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let mut args = vec!["fetch", "--progress", "--all", "--tags"];
    if prune {
        args.push("--prune");
    }
    let output = cli::run(workdir, &args, cancel, on_progress)?;
    Ok(summarize(&output.stderr, "Already up to date"))
}

pub fn pull(
    workdir: &Path,
    mode: PullMode,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let mut args = vec!["pull", "--progress"];
    match mode {
        PullMode::Merge => args.push("--no-rebase"),
        PullMode::Rebase => args.push("--rebase"),
        PullMode::FastForwardOnly => args.push("--ff-only"),
    }
    let output = cli::run(workdir, &args, cancel, on_progress)?;
    let text = format!("{}{}", output.stdout, output.stderr);
    Ok(summarize(&text, "Already up to date"))
}

pub fn push(
    workdir: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
    mode: PushMode,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let mut args = vec!["push", "--progress"];
    if mode == PushMode::ForceWithLease {
        args.push("--force-with-lease");
    }
    if set_upstream {
        args.push("--set-upstream");
    }
    args.push(remote);
    args.push(branch);

    let output = cli::run(workdir, &args, cancel, on_progress)?;
    let text = format!("{}{}", output.stdout, output.stderr);
    Ok(summarize(&text, "Everything up to date"))
}

pub fn push_tag(
    workdir: &Path,
    remote: &str,
    tag: &str,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let refspec = format!("refs/tags/{tag}");
    let args = vec!["push", "--progress", remote, refspec.as_str()];
    let output = cli::run(workdir, &args, cancel, on_progress)?;
    Ok(summarize(&output.stderr, "Tag already pushed"))
}

/// Clone `url` into `parent/name`.
pub fn clone(
    parent: &Path,
    url: &str,
    name: &str,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<std::path::PathBuf> {
    let args = vec!["clone", "--progress", url, name];
    cli::run(parent, &args, cancel, on_progress)?;
    Ok(parent.join(name))
}

/// The directory name `git clone` would choose for a URL.
///
/// Mirrors git's own rule: the last path segment, minus a trailing `.git`.
pub fn default_clone_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    // `git@host:owner/repo.git` has no slash after the colon in the scp form.
    let tail = trimmed
        .rsplit(['/', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed);
    tail.strip_suffix(".git").unwrap_or(tail).to_owned()
}

pub fn add_remote(workdir: &Path, name: &str, url: &str, cancel: &Cancel) -> Result<()> {
    cli::run(workdir, &["remote", "add", name, url], cancel, |_| {})?;
    Ok(())
}

pub fn remove_remote(workdir: &Path, name: &str, cancel: &Cancel) -> Result<()> {
    cli::run(workdir, &["remote", "remove", name], cancel, |_| {})?;
    Ok(())
}

pub fn set_remote_url(workdir: &Path, name: &str, url: &str, cancel: &Cancel) -> Result<()> {
    cli::run(workdir, &["remote", "set-url", name, url], cancel, |_| {})?;
    Ok(())
}

/// Condense git's chatter into one line for a toast.
///
/// Git says a great deal on stderr that is not interesting once the operation
/// succeeded; the last substantive line is almost always the outcome.
fn summarize(text: &str, fallback: &str) -> String {
    let interesting = text.lines().map(str::trim).rfind(|line| {
        !line.is_empty()
            && !line.starts_with("remote:")
            && !line.contains('%')
            && !line.starts_with("From ")
            && !line.starts_with("To ")
    });

    match interesting {
        Some(line) => line.to_owned(),
        None => fallback.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_names_match_gits_own_rule() {
        assert_eq!(
            default_clone_name("https://github.com/user/repo.git"),
            "repo"
        );
        assert_eq!(default_clone_name("https://github.com/user/repo"), "repo");
        assert_eq!(default_clone_name("git@github.com:user/repo.git"), "repo");
        assert_eq!(default_clone_name("https://example.com/a/b/c.git/"), "c");
        assert_eq!(default_clone_name("/local/path/project"), "project");
    }

    #[test]
    fn summaries_skip_progress_chatter() {
        let text = "remote: Counting objects: 100% (5/5)\n\
                    Receiving objects: 100% (5/5), done.\n\
                    From github.com:user/repo\n\
                    Fast-forward\n";
        assert_eq!(summarize(text, "fallback"), "Fast-forward");
    }

    #[test]
    fn an_empty_summary_falls_back() {
        assert_eq!(summarize("remote: something\n", "Up to date"), "Up to date");
        assert_eq!(summarize("", "Up to date"), "Up to date");
    }
}
