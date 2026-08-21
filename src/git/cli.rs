//! Running the real `git` binary.
//!
//! Network operations go through git itself rather than libgit2, and that is
//! the single most consequential decision in this codebase. libgit2 does not
//! implement `credential.helper`, does not read `~/.ssh/config`, and does not
//! know about the macOS keychain or Git Credential Manager. A client built on
//! it either reimplements all of that badly or asks the user to paste a token —
//! which is exactly the complaint GitAhead accumulated for years.
//!
//! Shelling out inherits every one of those mechanisms for free, already
//! configured, already working in the user's terminal.
//!
//! The cost is that a subprocess is a visible thing on Windows: without
//! [`git_command`] suppressing it, every fetch, pull and push would flash a
//! console window over the interface.

use crate::error::{Error, Result};
use crate::job::{Cancel, Progress};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// A `git` invocation with the console window suppressed.
///
/// On Windows a child process gets its own console unless `CREATE_NO_WINDOW`
/// says otherwise, and a GUI that flashes a black box on every fetch looks
/// broken. The flag does not exist on other platforms, where this is a plain
/// `Command::new`.
pub fn git_command() -> Command {
    let command = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // winbase.h: CREATE_NO_WINDOW.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        return command;
    }
    #[cfg(not(windows))]
    command
}

/// Output of a completed git invocation.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

/// Run git in `workdir`, streaming progress to `on_progress`.
///
/// Callers pass `--progress` themselves where it applies: git suppresses
/// progress when stderr is not a terminal, which is always the case here, but
/// the flag is not valid for every subcommand.
///
/// Progress is parsed from stderr, which is where git writes it. Note that git
/// separates progress updates with carriage returns rather than newlines, so
/// the reader has to split on both — reading by lines would deliver the entire
/// operation as one enormous final "line".
pub fn run(
    workdir: &Path,
    args: &[&str],
    cancel: &Cancel,
    on_progress: impl FnMut(Progress),
) -> Result<Output> {
    run_with_env(workdir, args, &[], cancel, on_progress)
}

/// As [`run`], with extra environment variables.
///
/// Used by the rebase driver, which controls git's behaviour through
/// `GIT_SEQUENCE_EDITOR` and `GIT_EDITOR` rather than by trying to script an
/// interactive session.
pub fn run_with_env(
    workdir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    cancel: &Cancel,
    mut on_progress: impl FnMut(Progress),
) -> Result<Output> {
    let mut command = git_command();
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Without this, git can block forever waiting for a password on a
        // terminal that does not exist. Failing with a clear message beats an
        // operation that never finishes and cannot be cancelled.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .spawn()
        .map_err(|e| Error::GitUnavailable(format!("{e}. Is git installed and on your PATH?")))?;

    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");

    let mut stderr = String::new();
    let mut buffer = [0u8; 4096];
    let mut pending = String::new();

    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }

        match stderr_pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buffer[..n]);
                pending.push_str(&chunk);
                // Split on either separator; git uses `\r` for in-place updates
                // and `\n` when a phase completes.
                while let Some(index) = pending.find(['\r', '\n']) {
                    let line: String = pending.drain(..=index).collect();
                    let line = line.trim_end_matches(['\r', '\n']);
                    if line.is_empty() {
                        continue;
                    }
                    stderr.push_str(line);
                    stderr.push('\n');
                    if let Some(progress) = parse_progress(line) {
                        on_progress(progress);
                    }
                }
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }
    if !pending.trim().is_empty() {
        stderr.push_str(pending.trim_end());
        stderr.push('\n');
    }

    let mut stdout = String::new();
    let _ = stdout_pipe.read_to_string(&mut stdout);
    let status = child.wait()?;

    if !status.success() {
        return Err(Error::GitCommand {
            args: args.join(" "),
            stderr: humanize(&stderr),
        });
    }

    Ok(Output { stdout, stderr })
}

/// Rewrite git's less helpful failures into something actionable.
fn humanize(stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if lower.contains("terminal prompts disabled") || lower.contains("could not read username") {
        return format!(
            "Authentication failed — no stored credentials for this remote.\n\
             Set up a credential helper or an SSH key, then try again.\n\n{stderr}"
        );
    }
    if lower.contains("permission denied (publickey)") {
        return format!(
            "The server rejected your SSH key.\n\
             Check that the right key is loaded (`ssh-add -l`).\n\n{stderr}"
        );
    }
    if lower.contains("could not resolve host") {
        return format!("Couldn't reach the remote — check your network.\n\n{stderr}");
    }
    stderr.to_owned()
}

/// Parse one of git's progress lines.
///
/// The shapes worth recognizing are `Counting objects: 45% (123/273)` and
/// `Receiving objects: 100% (273/273), done.`, optionally prefixed by
/// `remote: ` when the phase is happening on the server.
fn parse_progress(line: &str) -> Option<Progress> {
    let line = line.trim();
    let line = line.strip_prefix("remote: ").unwrap_or(line);
    let (label, rest) = line.split_once(':')?;

    let label = label.trim();
    if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }

    // `(done/total)` is the authoritative pair; the percentage before it is
    // redundant and rounds.
    let counts = rest.split_once('(').and_then(|(_, tail)| {
        let (inside, _) = tail.split_once(')')?;
        let (done, total) = inside.split_once('/')?;
        Some((
            done.trim().parse::<u64>().ok()?,
            total.trim().parse::<u64>().ok()?,
        ))
    });

    match counts {
        Some((done, total)) => Some(Progress {
            label: label.to_owned(),
            done,
            total: Some(total),
        }),
        None => {
            // A phase with no counts, e.g. `Resolving deltas: done.` — still
            // worth showing so the user sees motion.
            let percent = rest.trim().strip_suffix('%')?.trim().parse::<u64>().ok()?;
            Some(Progress {
                label: label.to_owned(),
                done: percent,
                total: Some(100),
            })
        }
    }
}

/// Whether the `git` binary can be found and run.
pub fn available() -> bool {
    git_command()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_progress_is_parsed() {
        let p = parse_progress("Receiving objects:  67% (183/273), 1.2 MiB | 500 KiB/s")
            .expect("progress");
        assert_eq!(p.label, "Receiving objects");
        assert_eq!(p.done, 183);
        assert_eq!(p.total, Some(273));
        assert!((p.fraction().unwrap() - 183.0 / 273.0).abs() < 0.001);
    }

    #[test]
    fn remote_prefixed_progress_is_parsed() {
        let p = parse_progress("remote: Counting objects: 45% (123/273)").expect("progress");
        assert_eq!(p.label, "Counting objects");
        assert_eq!(p.done, 123);
    }

    #[test]
    fn percentage_only_progress_is_parsed() {
        let p = parse_progress("Checking out files:  30%").expect("progress");
        assert_eq!(p.done, 30);
        assert_eq!(p.total, Some(100));
    }

    #[test]
    fn ordinary_messages_are_not_progress() {
        assert!(parse_progress("fatal: repository not found").is_none());
        assert!(parse_progress("To github.com:user/repo.git").is_none());
        assert!(parse_progress("").is_none());
    }

    #[test]
    fn authentication_failures_get_an_actionable_message() {
        let text = humanize("fatal: could not read Username for 'https://github.com'");
        assert!(text.contains("Authentication failed"));
        assert!(text.contains("credential helper"));
    }

    #[test]
    fn the_git_binary_is_available_in_this_environment() {
        assert!(available(), "these tests assume git is installed");
    }
}
