//! Rebase, including the interactive kind.
//!
//! Driven through the `git` binary rather than libgit2's `Rebase` API, because
//! libgit2 only knows how to replay commits one at a time: it has no notion of
//! squash, fixup, or reorder, so an interactive rebase built on it would mean
//! reimplementing the todo semantics from scratch.
//!
//! The interactive part avoids git's editor protocol entirely. Rather than
//! trying to impersonate an editor across an unpredictable number of prompts,
//! the todo list is supplied directly through `GIT_SEQUENCE_EDITOR`, and
//! rewording is expressed as an `exec git commit --amend -F <file>` line after
//! the pick it applies to. Nothing has to be typed, and nothing depends on the
//! order git happens to open editors in.

use crate::error::{Error, Result};
use crate::git::cli;
use crate::job::{Cancel, Progress};
use git2::{Oid, Repository};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAction {
    /// Keep the commit as it is.
    Pick,
    /// Keep it, with a new message.
    Reword,
    /// Fold into the previous commit, combining the messages.
    Squash,
    /// Fold into the previous commit, discarding this message.
    Fixup,
    /// Leave it out entirely.
    Drop,
}

impl StepAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pick => "Pick",
            Self::Reword => "Reword",
            Self::Squash => "Squash",
            Self::Fixup => "Fixup",
            Self::Drop => "Drop",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Pick => "Keep this commit unchanged",
            Self::Reword => "Keep the changes, write a new message",
            Self::Squash => "Combine into the commit above, keeping both messages",
            Self::Fixup => "Combine into the commit above, discarding this message",
            Self::Drop => "Remove this commit entirely",
        }
    }

    pub fn all() -> [Self; 5] {
        [
            Self::Pick,
            Self::Reword,
            Self::Squash,
            Self::Fixup,
            Self::Drop,
        ]
    }

    fn keyword(self) -> &'static str {
        match self {
            Self::Pick | Self::Reword => "pick",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseStep {
    pub oid: Oid,
    pub short_id: String,
    pub summary: String,
    pub action: StepAction,
    /// New message, for `Reword`.
    pub message: String,
}

/// A rebase plan: the commits to replay, oldest first, and where onto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebasePlan {
    /// Oldest first, matching the order git's todo list uses.
    pub steps: Vec<RebaseStep>,
    /// The commit the replay starts from — the parent of the oldest step.
    pub base: Oid,
}

impl RebasePlan {
    /// Whether the plan would change anything, given the order history had.
    pub fn is_noop(&self, original: &[Oid]) -> bool {
        self.steps.iter().all(|s| s.action == StepAction::Pick) && !self.reordered_against(original)
    }

    /// The reason this plan can't be run, if there is one.
    pub fn first_problem(&self) -> Option<&'static str> {
        // Squash and fixup fold into the commit *above*, so the first step has
        // nothing to fold into.
        match self.steps.first().map(|s| s.action) {
            Some(StepAction::Squash) | Some(StepAction::Fixup) => {
                Some("The first commit has nothing above it to combine into")
            }
            _ => {
                if self.steps.iter().all(|s| s.action == StepAction::Drop) {
                    Some("Dropping every commit would leave nothing to rebase")
                } else {
                    None
                }
            }
        }
    }

    /// Whether the steps have been moved relative to the original history.
    ///
    /// Compared against the order history had rather than tracked as a flag,
    /// so dragging a commit away and back again correctly reads as no change.
    fn reordered_against(&self, original: &[Oid]) -> bool {
        let current: Vec<Oid> = self.steps.iter().map(|s| s.oid).collect();
        let kept: Vec<Oid> = original
            .iter()
            .copied()
            .filter(|oid| current.contains(oid))
            .collect();
        kept != current
    }
}

/// Build the todo text git should use, plus any message files it references.
///
/// Returns the todo body and a list of `(filename, contents)` for reword
/// messages, which the caller writes into a scratch directory.
pub fn render_todo(plan: &RebasePlan) -> (String, Vec<(String, String)>) {
    let mut todo = String::new();
    let mut messages = Vec::new();

    for (index, step) in plan.steps.iter().enumerate() {
        if step.action == StepAction::Drop {
            // Omitted entirely; git treats an absent line as a drop, and
            // writing `drop` explicitly is equivalent but noisier.
            continue;
        }
        todo.push_str(&format!(
            "{} {} {}\n",
            step.action.keyword(),
            step.oid,
            step.summary
        ));

        if step.action == StepAction::Reword {
            let name = format!("msg-{index}");
            let mut text = step.message.trim().to_owned();
            if text.is_empty() {
                text = step.summary.clone();
            }
            text.push('\n');
            messages.push((name.clone(), text));
            // `exec` runs after the pick lands, so the amend applies to the
            // commit that was just created — no editor involved.
            todo.push_str(&format!(
                "exec git commit --amend --no-edit -F \"$GITUP_MSG_DIR/{name}\"\n"
            ));
        }
    }

    (todo, messages)
}

/// Rebase the current branch onto `onto`.
pub fn rebase_onto(
    workdir: &Path,
    onto: &str,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    let args = vec!["rebase", "--autostash", onto];
    let output = cli::run(workdir, &args, cancel, on_progress)?;
    Ok(summarize(&output.stderr, &output.stdout))
}

/// Run an interactive rebase from a plan.
pub fn rebase_interactive(
    workdir: &Path,
    plan: &RebasePlan,
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<String> {
    if let Some(problem) = plan.first_problem() {
        return Err(Error::refused(problem));
    }

    let (todo, messages) = render_todo(plan);
    let scratch = tempdir()?;
    let todo_path = scratch.join("todo");
    std::fs::write(&todo_path, &todo)?;
    for (name, text) in &messages {
        std::fs::write(scratch.join(name), text)?;
    }

    // git runs the sequence editor through a shell, so the path is quoted.
    let sequence_editor = format!("cp {}", shell_quote(&todo_path.to_string_lossy()));
    let base = plan.base.to_string();
    let args = vec!["rebase", "-i", "--autostash", base.as_str()];

    let result = cli::run_with_env(
        workdir,
        &args,
        &[
            ("GIT_SEQUENCE_EDITOR", sequence_editor.as_str()),
            // Squash and fixup still ask for a combined message; `true` accepts
            // whatever git prepared, which is the conventional result.
            ("GIT_EDITOR", "true"),
            ("GITUP_MSG_DIR", &scratch.to_string_lossy()),
        ],
        cancel,
        on_progress,
    );

    // Message files are only needed while git is running.
    let _ = std::fs::remove_dir_all(&scratch);

    let output = result?;
    Ok(summarize(&output.stderr, &output.stdout))
}

pub fn continue_rebase(workdir: &Path, cancel: &Cancel) -> Result<String> {
    let output = cli::run_with_env(
        workdir,
        &["rebase", "--continue"],
        &[("GIT_EDITOR", "true")],
        cancel,
        |_| {},
    )?;
    Ok(summarize(&output.stderr, &output.stdout))
}

pub fn skip(workdir: &Path, cancel: &Cancel) -> Result<String> {
    let output = cli::run(workdir, &["rebase", "--skip"], cancel, |_| {})?;
    Ok(summarize(&output.stderr, &output.stdout))
}

pub fn abort(workdir: &Path, cancel: &Cancel) -> Result<String> {
    cli::run(workdir, &["rebase", "--abort"], cancel, |_| {})?;
    Ok("Rebase aborted".to_owned())
}

/// Build a plan for the commits between `base` and HEAD.
pub fn plan_from(repo: &Repository, base: Oid) -> Result<RebasePlan> {
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.hide(base)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL)?;

    let mut steps = Vec::new();
    for oid in walk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        if commit.parent_count() > 1 {
            return Err(Error::refused(
                "This range contains a merge commit, which an interactive rebase can't replay",
            ));
        }
        steps.push(RebaseStep {
            oid,
            short_id: super::repo::short_id(oid),
            summary: commit.summary().ok().flatten().unwrap_or("").to_owned(),
            action: StepAction::Pick,
            message: String::new(),
        });
    }

    if steps.is_empty() {
        return Err(Error::refused(
            "There are no commits to rebase in that range",
        ));
    }
    // git's todo list is oldest first; the walk gives newest first.
    steps.reverse();
    Ok(RebasePlan { steps, base })
}

/// Whether a plan reorders commits relative to how they appear in history.
pub fn is_reordered(plan: &RebasePlan, original: &[Oid]) -> bool {
    plan.reordered_against(original)
}

/// A scratch directory for one rebase's todo and message files.
///
/// The counter matters: several rebases can be in flight in one process — the
/// test suite runs them in parallel — and a directory keyed only by process id
/// would have them overwriting each other's todo lists.
fn tempdir() -> Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("gitup-rebase-{}-{serial}", std::process::id()));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

/// Quote a path for a shell command line.
fn shell_quote(text: &str) -> String {
    // Single quotes disable everything except a literal single quote, which is
    // escaped by closing, inserting an escaped quote, and reopening.
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn summarize(stderr: &str, stdout: &str) -> String {
    for text in [stdout, stderr] {
        if let Some(line) = text
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty() && !l.contains('%'))
        {
            return line.to_owned();
        }
    }
    "Rebase complete".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(action: StepAction, summary: &str) -> RebaseStep {
        RebaseStep {
            oid: Oid::from_str("0123456789abcdef0123456789abcdef01234567").unwrap(),
            short_id: "0123456".to_owned(),
            summary: summary.to_owned(),
            action,
            message: String::new(),
        }
    }

    #[test]
    fn a_plain_plan_renders_pick_lines() {
        let plan = RebasePlan {
            steps: vec![
                step(StepAction::Pick, "First"),
                step(StepAction::Pick, "Second"),
            ],
            base: Oid::ZERO_SHA1,
        };
        let (todo, messages) = render_todo(&plan);
        assert_eq!(todo.lines().count(), 2);
        assert!(todo.lines().all(|l| l.starts_with("pick ")));
        assert!(messages.is_empty());
    }

    #[test]
    fn dropped_commits_are_omitted() {
        let plan = RebasePlan {
            steps: vec![
                step(StepAction::Pick, "Keep"),
                step(StepAction::Drop, "Remove"),
            ],
            base: Oid::ZERO_SHA1,
        };
        let (todo, _) = render_todo(&plan);
        assert_eq!(todo.lines().count(), 1);
        assert!(todo.contains("Keep"));
        assert!(!todo.contains("Remove"));
    }

    #[test]
    fn squash_and_fixup_use_their_own_keywords() {
        let plan = RebasePlan {
            steps: vec![
                step(StepAction::Pick, "Base"),
                step(StepAction::Squash, "Folded"),
                step(StepAction::Fixup, "Silent"),
            ],
            base: Oid::ZERO_SHA1,
        };
        let (todo, _) = render_todo(&plan);
        let lines: Vec<&str> = todo.lines().collect();
        assert!(lines[1].starts_with("squash "));
        assert!(lines[2].starts_with("fixup "));
    }

    #[test]
    fn reword_emits_an_exec_amend_with_a_message_file() {
        let mut reword = step(StepAction::Reword, "Old summary");
        reword.message = "A better message".to_owned();
        let plan = RebasePlan {
            steps: vec![reword],
            base: Oid::ZERO_SHA1,
        };
        let (todo, messages) = render_todo(&plan);

        assert!(todo.starts_with("pick "), "reword still picks first");
        assert!(
            todo.contains("exec git commit --amend --no-edit -F"),
            "got:\n{todo}"
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1, "A better message\n");
    }

    #[test]
    fn an_empty_reword_message_falls_back_to_the_original() {
        let plan = RebasePlan {
            steps: vec![step(StepAction::Reword, "Original summary")],
            base: Oid::ZERO_SHA1,
        };
        let (_, messages) = render_todo(&plan);
        assert_eq!(messages[0].1, "Original summary\n");
    }

    #[test]
    fn squashing_the_first_commit_is_refused() {
        let plan = RebasePlan {
            steps: vec![step(StepAction::Squash, "Nothing above me")],
            base: Oid::ZERO_SHA1,
        };
        assert!(plan.first_problem().is_some());
    }

    #[test]
    fn dropping_everything_is_refused() {
        let plan = RebasePlan {
            steps: vec![step(StepAction::Drop, "a"), step(StepAction::Drop, "b")],
            base: Oid::ZERO_SHA1,
        };
        assert!(plan.first_problem().is_some());
    }

    #[test]
    fn shell_quoting_survives_awkward_paths() {
        assert_eq!(shell_quote("/tmp/plain"), "'/tmp/plain'");
        assert_eq!(shell_quote("/tmp/with space"), "'/tmp/with space'");
        assert_eq!(
            shell_quote("/tmp/it's"),
            r"'/tmp/it'\''s'",
            "an embedded quote must not end the quoting"
        );
    }

    #[test]
    fn reordering_is_detected_against_the_original_order() {
        let a = Oid::from_str("1111111111111111111111111111111111111111").unwrap();
        let b = Oid::from_str("2222222222222222222222222222222222222222").unwrap();
        let mut first = step(StepAction::Pick, "a");
        first.oid = a;
        let mut second = step(StepAction::Pick, "b");
        second.oid = b;

        let same = RebasePlan {
            steps: vec![first.clone(), second.clone()],
            base: Oid::ZERO_SHA1,
        };
        assert!(!is_reordered(&same, &[a, b]));

        let swapped = RebasePlan {
            steps: vec![second, first],
            base: Oid::ZERO_SHA1,
        };
        assert!(is_reordered(&swapped, &[a, b]));
    }
}
