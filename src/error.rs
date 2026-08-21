//! Error type shared by every fallible operation in the app.
//!
//! Errors are surfaced to the user, not panicked on: a Git client that dies
//! because a repository is in an odd state is worse than one that says so.

use std::fmt;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Anything libgit2 reported.
    Git(git2::Error),
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The path isn't inside a Git repository.
    NotARepository(PathBuf),
    /// The `git` binary failed; carries its stderr.
    GitCommand { args: String, stderr: String },
    /// The `git` binary could not be found or run at all.
    GitUnavailable(String),
    /// The operation was superseded or cancelled; not shown to the user.
    Cancelled,
    /// Something the user needs to fix before this can work.
    Refused(String),
}

impl Error {
    pub fn refused(msg: impl Into<String>) -> Self {
        Self::Refused(msg.into())
    }

    /// Cancellation is bookkeeping, not something worth a toast.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// A short, human-facing sentence. Deliberately not the raw libgit2 text
    /// where we can do better.
    pub fn user_message(&self) -> String {
        match self {
            Self::NotARepository(p) => {
                format!("{} isn't inside a Git repository.", p.display())
            }
            Self::GitUnavailable(why) => {
                format!("Couldn't run the `git` command: {why}")
            }
            Self::GitCommand { stderr, .. } => summarize_git_error(stderr),
            Self::Cancelled => "Cancelled.".to_owned(),
            Self::Refused(m) => m.clone(),
            Self::Git(e) => capitalize(e.message()),
            Self::Io(e) => e.to_string(),
        }
    }
}

/// Pull the meaningful part out of git's output.
///
/// git buries the answer in different places depending on how it failed. A
/// hard failure is marked — `fatal:`, `error:`, a `!` in the refspec table, or
/// `CONFLICT` — and everything from that marker on is worth keeping. A *soft*
/// refusal has no marker at all and leads with the explanation, then trails
/// off into example commands: taking the last line there reports
/// `git branch --set-upstream-to=<remote>/<branch> main` as the error, which
/// tells the user nothing about what went wrong.
fn summarize_git_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    // A marked failure: keep the explanation and everything after it, so the
    // hint git appends survives.
    if let Some(index) = lines.iter().position(|l| {
        l.starts_with("fatal:")
            || l.starts_with("error:")
            || l.starts_with("! [")
            || l.starts_with("CONFLICT")
    }) {
        return lines[index..].join("\n");
    }

    // Otherwise the first line that is not routine chatter.
    if let Some(line) = lines.iter().find(|l| !is_noise(l)) {
        return line.to_string();
    }
    if let Some(line) = lines.iter().find(|l| l.starts_with("hint:")) {
        return line.to_string();
    }
    lines
        .last()
        .map(|l| l.to_string())
        .unwrap_or_else(|| "git exited with an error".to_owned())
}

/// Lines git prints as a matter of course, which never explain a failure.
fn is_noise(line: &str) -> bool {
    line.starts_with("From ")
        || line.starts_with("To ")
        || line.starts_with("remote:")
        || line.starts_with("Auto-merging")
        || line.starts_with("Unpacking objects")
        || line.starts_with("Receiving objects")
        || line.starts_with("Resolving deltas")
        || line.starts_with("Counting objects")
        || line.starts_with("Enumerating objects")
        || line.contains('%')
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for Error {}

impl From<git2::Error> for Error {
    fn from(e: git2::Error) -> Self {
        Self::Git(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::summarize_git_error;

    #[test]
    fn a_rejected_push_reports_the_rejection_not_the_url() {
        let stderr = "To /tmp/origin\n\
                      ! [rejected]        main -> main (fetch first)\n\
                      error: failed to push some refs to '/tmp/origin'\n\
                      hint: Updates were rejected because the remote contains work\n";
        let message = summarize_git_error(stderr);
        assert!(message.starts_with("! [rejected]"), "got {message:?}");
        assert!(message.contains("failed to push"));
        assert!(!message.contains("To /tmp/origin"));
    }

    #[test]
    fn a_fatal_line_wins_over_earlier_chatter() {
        let stderr = "Cloning into 'x'...\nfatal: repository not found\n";
        assert_eq!(summarize_git_error(stderr), "fatal: repository not found");
    }

    #[test]
    fn an_unmarked_refusal_reports_its_first_line_not_its_last() {
        // git's actual output when a branch has no upstream. The last line is
        // an example command, which is not the reason for anything.
        let stderr = "There is no tracking information for the current branch.\n\
                      Please specify which branch you want to merge with.\n\
                      See git-pull(1) for details.\n\
                      \n\
                          git pull <remote> <branch>\n\
                      \n\
                      If you wish to set tracking information for this branch you can do so with:\n\
                      \n\
                          git branch --set-upstream-to=<remote>/<branch> main\n";
        assert_eq!(
            summarize_git_error(stderr),
            "There is no tracking information for the current branch."
        );
    }

    #[test]
    fn routine_chatter_is_skipped() {
        let stderr = "From /tmp/origin\n\
                      Auto-merging shared.txt\n\
                      Something went wrong\n";
        assert_eq!(summarize_git_error(stderr), "Something went wrong");
    }

    #[test]
    fn a_merge_conflict_reports_the_conflict_and_what_follows() {
        let stderr = "Auto-merging shared.txt\n\
                      CONFLICT (content): Merge conflict in shared.txt\n\
                      Automatic merge failed; fix conflicts and then commit the result.\n";
        let message = summarize_git_error(stderr);
        assert!(message.starts_with("CONFLICT"), "got {message:?}");
        assert!(message.contains("Automatic merge failed"));
    }

    #[test]
    fn empty_output_still_says_something() {
        assert_eq!(summarize_git_error(""), "git exited with an error");
    }
}
