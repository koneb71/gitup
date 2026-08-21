//! Worker threads. Each owns its own `git2::Repository` handles.
//!
//! `git2::Repository` is `Send` but not `Sync`, so handles are never shared
//! between threads. Each worker keeps its own cache keyed by repository path;
//! opening a repository costs a few milliseconds, and a client re-reads status
//! constantly, so caching is what keeps refreshes cheap.

use super::{Job, Message, Mutation, Outcome, PartialKind, Task};
use crate::error::{Error, Result};
use crate::git::commit;
use crate::git::stage;
use crate::git::{
    blame, branch, conflict, identity, merge, rebase, remote, search, stash, submodule,
};
use crate::git::{diff, graph, refs, repo, status, RepoKey};
use crossbeam_channel::{Receiver, Sender};
use git2::Repository;
use std::collections::HashMap;
use std::thread::JoinHandle;

/// Per-thread repository handles.
#[derive(Default)]
struct RepoCache {
    open: HashMap<RepoKey, Repository>,
}

impl RepoCache {
    fn get(&mut self, key: &RepoKey) -> Result<&Repository> {
        self.ensure(key)?;
        // The insert above guarantees presence.
        Ok(self.open.get(key).expect("just inserted"))
    }

    /// Open the repository if needed, and make sure its cached index reflects
    /// what is on disk.
    ///
    /// Every worker holds its own `Repository`, and libgit2 caches the index
    /// inside each one. When the write worker stages something, a read worker's
    /// cached index is stale until it re-reads — so the UI would show the
    /// state from before the click. `read(false)` is a no-op when the file
    /// hasn't changed, so this costs a stat call.
    fn ensure(&mut self, key: &RepoKey) -> Result<()> {
        if !self.open.contains_key(key) {
            let repo = repo::open(key)?;
            self.open.insert(key.clone(), repo);
            return Ok(());
        }
        if let Some(repo) = self.open.get(key) {
            if let Ok(mut index) = repo.index() {
                let _ = index.read(false);
            }
        }
        Ok(())
    }

    /// Some libgit2 operations — `stash_foreach`, for one — need a mutable
    /// handle even though they only read.
    fn get_mut(&mut self, key: &RepoKey) -> Result<&mut Repository> {
        self.ensure(key)?;
        Ok(self.open.get_mut(key).expect("just inserted"))
    }
}

pub(crate) fn spawn(
    name: String,
    rx: Receiver<Task>,
    tx: Sender<Message>,
    ctx: egui::Context,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let mut cache = RepoCache::default();
            // Exits when the JobSystem drops its senders.
            while let Ok(task) = rx.recv() {
                let Task {
                    id,
                    topic,
                    job,
                    cancel,
                } = task;

                if cancel.is_cancelled() {
                    continue;
                }

                let report = Reporter {
                    tx: &tx,
                    ctx: &ctx,
                    id,
                    topic: topic.clone(),
                };
                let result = run(&mut cache, job, &cancel, &report);

                let msg = match result {
                    Ok(outcome) => Message::Done { id, topic, outcome },
                    Err(error) => Message::Failed { id, topic, error },
                };

                if tx.send(msg).is_err() {
                    break;
                }
                // Without this the result sits in the channel until something
                // else happens to trigger a frame.
                ctx.request_repaint();
            }
        })
        .expect("failed to spawn worker thread")
}

fn run(
    cache: &mut RepoCache,
    job: Job,
    cancel: &super::Cancel,
    report: &Reporter<'_>,
) -> Result<Outcome> {
    match job {
        Job::OpenRepo { path, token } => {
            let key = repo::discover(&path)?;
            let r = cache.get(&key)?;
            let head = repo::head_info(r)?;
            let git_dir = r.path().to_path_buf();
            Ok(Outcome::RepoOpened {
                token,
                key,
                head: Box::new(head),
                git_dir,
            })
        }

        Job::ReadHead(key) => {
            let r = cache.get(&key)?;
            let head = repo::head_info(r)?;
            Ok(Outcome::Head {
                key,
                head: Box::new(head),
            })
        }

        Job::ReadStatus {
            repo: key,
            include_ignored,
        } => {
            let r = cache.get(&key)?;
            let snapshot = status::status(r, include_ignored)?;
            Ok(Outcome::Status {
                key,
                status: snapshot,
            })
        }

        Job::LoadGraph { repo: key, limit } => {
            let r = cache.get(&key)?;
            let page = graph::build(r, limit, cancel)?;
            Ok(Outcome::Graph { key, page, limit })
        }

        Job::Mutate { repo: key, action } => {
            let default_message = action.success_message();
            let moved_head = action.moves_head();
            // Network operations report git's own summary; everything else has
            // a fixed past-tense description.
            let reported = apply_mutation(cache, &key, action, cancel, report)?;
            Ok(Outcome::Mutated {
                key,
                message: reported.unwrap_or(default_message),
                moved_head,
            })
        }

        Job::LoadConflicts(key) => {
            let r = cache.get(&key)?;
            let conflicts = conflict::list(r)?;
            Ok(Outcome::Conflicts { key, conflicts })
        }

        Job::LoadSubmodules(key) => {
            let r = cache.get(&key)?;
            let submodules = submodule::list(r)?;
            Ok(Outcome::Submodules { key, submodules })
        }

        Job::ReadIdentity { repo } => {
            let identities = match &repo {
                Some(key) => identity::read(cache.get(key)?)?,
                // Nothing open: the global config is still the user's, and is
                // still worth being able to fix from the settings sheet.
                None => identity::read_global()?,
            };
            Ok(Outcome::Identity {
                key: repo,
                identities: Box::new(identities),
            })
        }

        Job::SetIdentity {
            repo,
            scope,
            identity,
        } => {
            match &repo {
                Some(key) => identity::write(Some(cache.get(key)?), scope, &identity)?,
                None => identity::write(None, scope, &identity)?,
            }
            // Read back rather than echoing what was asked for: the value that
            // matters is the one now in force, which a level further down the
            // chain can still be overriding.
            let identities = match &repo {
                Some(key) => identity::read(cache.get(key)?)?,
                None => identity::read_global()?,
            };
            Ok(Outcome::Identity {
                key: repo,
                identities: Box::new(identities),
            })
        }

        Job::Blame {
            repo: key,
            path,
            at,
            theme,
        } => {
            let r = cache.get(&key)?;
            let result = blame::blame(r, &path, at, theme, cancel)?;
            Ok(Outcome::Blame { key, result })
        }

        Job::Search {
            repo: key,
            kind,
            query,
            limit,
        } => {
            let r = cache.get(&key)?;
            let results = search::search(r, kind, &query, limit, cancel)?;
            Ok(Outcome::Search { key, results, kind })
        }

        Job::FileHistory {
            repo: key,
            path,
            limit,
        } => {
            let r = cache.get(&key)?;
            let results = search::file_history(r, &path, limit, cancel)?;
            Ok(Outcome::FileHistory { key, results, path })
        }

        Job::Clone { url, parent, name } => {
            let path = remote::clone(&parent, &url, &name, cancel, |p| report.progress(p))?;
            Ok(Outcome::Cloned { path })
        }

        Job::LoadRefs(key) => {
            let r = cache.get_mut(&key)?;
            let tree = refs::build(r)?;
            Ok(Outcome::Refs { key, tree })
        }

        Job::LoadDiff {
            repo: key,
            target,
            theme,
        } => {
            let r = cache.get(&key)?;
            let model = diff::build(r, target, theme, cancel)?;
            Ok(Outcome::Diff { key, target, model })
        }
    }
}

/// Perform a mutation, returning a message to show when the operation supplies
/// its own (network commands do; local ones don't).
fn apply_mutation(
    cache: &mut RepoCache,
    key: &RepoKey,
    action: Mutation,
    cancel: &super::Cancel,
    report: &Reporter<'_>,
) -> Result<Option<String>> {
    // Stash needs a mutable handle; network commands need only a path.
    match action {
        Mutation::StashSave {
            message,
            include_untracked,
        } => {
            let r = cache.get_mut(key)?;
            stash::save(r, message.as_deref(), include_untracked, false)?;
            return Ok(None);
        }
        Mutation::StashApply(index) => {
            let r = cache.get_mut(key)?;
            stash::apply(r, index)?;
            return Ok(None);
        }
        Mutation::StashPop(index) => {
            let r = cache.get_mut(key)?;
            stash::pop(r, index)?;
            return Ok(None);
        }
        Mutation::StashDrop(index) => {
            let r = cache.get_mut(key)?;
            stash::drop(r, index)?;
            return Ok(None);
        }

        Mutation::Fetch {
            remote: name,
            prune,
        } => {
            let workdir = key.path().to_path_buf();
            let summary = match name {
                Some(name) => {
                    remote::fetch(&workdir, &name, prune, cancel, |p| report.progress(p))?
                }
                None => remote::fetch_all(&workdir, prune, cancel, |p| report.progress(p))?,
            };
            return Ok(Some(summary));
        }
        Mutation::Pull(mode) => {
            let workdir = key.path().to_path_buf();
            let summary = remote::pull(&workdir, mode, cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::Push {
            remote: name,
            branch: branch_name,
            set_upstream,
            mode,
        } => {
            let workdir = key.path().to_path_buf();
            let summary = remote::push(
                &workdir,
                &name,
                &branch_name,
                set_upstream,
                mode,
                cancel,
                |p| report.progress(p),
            )?;
            return Ok(Some(summary));
        }
        Mutation::PushTag { remote: name, tag } => {
            let workdir = key.path().to_path_buf();
            let summary = remote::push_tag(&workdir, &name, &tag, cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::RebaseOnto(onto) => {
            let workdir = key.path().to_path_buf();
            let summary = rebase::rebase_onto(&workdir, &onto, cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::RebaseInteractive(plan) => {
            let workdir = key.path().to_path_buf();
            let summary =
                rebase::rebase_interactive(&workdir, &plan, cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::RebaseContinue => {
            return Ok(Some(rebase::continue_rebase(key.path(), cancel)?));
        }
        Mutation::RebaseSkip => {
            return Ok(Some(rebase::skip(key.path(), cancel)?));
        }
        Mutation::RebaseAbort => {
            rebase::abort(key.path(), cancel)?;
            return Ok(None);
        }

        Mutation::UpdateSubmodule(path) => {
            let workdir = key.path().to_path_buf();
            let summary =
                submodule::update(&workdir, path.as_deref(), cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::AddSubmodule { url, path } => {
            let workdir = key.path().to_path_buf();
            let summary = submodule::add(&workdir, &url, &path, cancel, |p| report.progress(p))?;
            return Ok(Some(summary));
        }
        Mutation::RemoveSubmodule(path) => {
            return Ok(Some(submodule::remove(key.path(), &path, cancel)?));
        }

        Mutation::AddRemote { name, url } => {
            remote::add_remote(key.path(), &name, &url, cancel)?;
            return Ok(None);
        }
        Mutation::RemoveRemote(name) => {
            remote::remove_remote(key.path(), &name, cancel)?;
            return Ok(None);
        }
        _ => {}
    }

    let repo = cache.get(key)?;
    apply_local(repo, action).map(|_| None)
}

fn apply_local(repo: &Repository, action: Mutation) -> Result<()> {
    match action {
        Mutation::StageFiles(paths) => stage::stage_files(repo, &paths),
        Mutation::UnstageFiles(paths) => stage::unstage_files(repo, &paths),
        Mutation::DiscardFiles(paths) => stage::discard_files(repo, &paths),
        Mutation::DeleteUntracked(paths) => stage::delete_untracked(repo, &paths),
        Mutation::StageAll => stage::stage_all(repo),

        Mutation::Checkout(name) => branch::checkout_branch(repo, &name),
        Mutation::CheckoutRemote(name) => branch::checkout_remote(repo, &name).map(|_| ()),
        Mutation::CheckoutCommit(oid) => branch::checkout_commit(repo, oid),
        Mutation::CreateBranch {
            name,
            start_point,
            checkout,
        } => branch::create(repo, &name, start_point.as_deref(), checkout),
        Mutation::DeleteBranch { name, force } => branch::delete(repo, &name, force),
        Mutation::RenameBranch { old, new } => branch::rename(repo, &old, &new, false),
        Mutation::SetUpstream {
            branch: b,
            upstream,
        } => branch::set_upstream(repo, &b, upstream.as_deref()),
        Mutation::CreateTag {
            name,
            target,
            message,
        } => branch::create_tag(repo, &name, target, message.as_deref()),
        Mutation::DeleteTag(name) => branch::delete_tag(repo, &name),

        Mutation::Partial {
            model,
            path,
            selections,
            kind,
        } => {
            // The diff may have been recomputed since the click; if the file is
            // gone from it, the change it described no longer exists.
            let file = model
                .find(&path)
                .ok_or_else(|| Error::refused(format!("{path} is no longer part of this diff")))?;
            match kind {
                PartialKind::Stage => stage::stage_partial(repo, file, &selections),
                PartialKind::Unstage => stage::unstage_partial(repo, file, &selections),
                PartialKind::Discard => stage::discard_partial(repo, file, &selections),
            }
        }

        Mutation::Commit { message, mode } => commit::commit(repo, &message, mode).map(|_| ()),

        Mutation::Merge(rev) => merge::merge(repo, &rev).map(|_| ()),
        Mutation::CherryPick { oid, mainline } => {
            merge::cherry_pick(repo, oid, mainline).map(|_| ())
        }
        Mutation::Revert { oid, mainline } => merge::revert(repo, oid, mainline).map(|_| ()),
        Mutation::Reset { oid, kind } => merge::reset(repo, oid, kind),
        Mutation::AbortOperation => merge::abort(repo),
        Mutation::ResolveConflict { path, resolution } => {
            conflict::resolve_with(repo, &path, resolution)
        }
        Mutation::ResolveConflictContent { path, content } => {
            conflict::resolve_with_content(repo, &path, &content)
        }

        // Handled before this function is reached.
        other => Err(Error::refused(format!(
            "internal: {} was not dispatched",
            other.success_message()
        ))),
    }
}

/// Lets a running job stream progress back to the UI.
pub(crate) struct Reporter<'a> {
    tx: &'a Sender<Message>,
    ctx: &'a egui::Context,
    id: u64,
    topic: super::Topic,
}

impl Reporter<'_> {
    fn progress(&self, progress: crate::job::Progress) {
        let _ = self.tx.send(Message::Progress {
            id: self.id,
            topic: self.topic.clone(),
            progress,
        });
        self.ctx.request_repaint();
    }
}

/// Kept for the error path in higher layers that need to name a missing repo.
#[allow(dead_code)]
fn not_open(key: &RepoKey) -> Error {
    Error::NotARepository(key.0.clone())
}
