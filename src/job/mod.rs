//! The job system: the boundary between the UI thread and every Git operation.
//!
//! egui redraws on the UI thread every frame, so a blocking libgit2 call —
//! a revwalk over a large history, a status scan, a fetch — would freeze the
//! window. The rule this module exists to enforce is absolute:
//!
//! > **The UI thread never calls libgit2.**
//!
//! The UI dispatches [`Job`]s and, on later frames, polls for [`Message`]s.
//! Nothing blocks.
//!
//! ## Supersession
//!
//! Most jobs answer a question the UI is *currently* asking: "what is the diff
//! for the selected commit?". If the user clicks a different commit before the
//! answer arrives, that answer is worthless. Each job therefore carries a
//! [`Topic`], and dispatching on a topic supersedes anything still in flight for
//! it: the older job is signalled to cancel and its result is dropped on arrival.
//! Without this the diff pane flickers between commits under fast scrolling.
//!
//! Mutations use [`Topic::Unique`] and are never superseded — a commit that got
//! halfway through must not be abandoned because the user clicked elsewhere.

pub mod worker;

use crate::error::Error;
use crate::git::blame::BlameResult;
use crate::git::commit::CommitMode;
use crate::git::conflict::{Conflicts, Resolution};
use crate::git::highlight::HighlightTheme;
use crate::git::merge::ResetKind;
use crate::git::rebase::RebasePlan;
use crate::git::search::{SearchKind, SearchResults};
use crate::git::stage::HunkSelection;
use crate::git::submodule::Submodules;
use crate::git::{DiffModel, DiffTarget, GraphPage, HeadInfo, RefTree, RepoKey, StatusSnapshot};
use crossbeam_channel::{Receiver, Sender};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Groups jobs that answer the same question, so a newer one can supersede an
/// older one.
/// Which diff the UI is asking about.
///
/// These are separate topics because the working-tree view needs the staged and
/// unstaged diffs at the same time; a single `Diff` topic would make each
/// request cancel the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSlot {
    Commit,
    Staged,
    Unstaged,
}

impl DiffSlot {
    pub fn of(target: DiffTarget) -> Self {
        match target {
            DiffTarget::Commit(_) => Self::Commit,
            DiffTarget::Staged => Self::Staged,
            DiffTarget::Unstaged => Self::Unstaged,
        }
    }
}

/// The kind of question a job answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopicKind {
    Head,
    Status,
    Graph,
    Diff(DiffSlot),
    Refs,
    Conflicts,
    Submodules,
    Identity,
    Blame,
    Search,
    /// Never superseded. Used for mutations and for anything whose result must
    /// arrive even if the user has moved on.
    Unique(u64),
}

/// What a job is about: a kind of question, and which repository it is about.
///
/// The repository is part of the identity, not decoration. Supersession works
/// by replacing the outstanding job on a topic — so a topic shared between two
/// repositories means loading the history of one *cancels* loading the history
/// of the other. With several tabs open that is not a subtle race: every tab
/// but the last is left permanently blank.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Topic {
    kind: TopicKind,
    /// `None` for work not tied to a repository, such as a clone.
    scope: Option<RepoKey>,
    /// The tab this belongs to, for work that has no repository to name yet.
    ///
    /// Opening is the case that needs it: until the repository is found there
    /// is no key, so a failure could otherwise only be blamed on whichever tab
    /// happens to be visible when it lands.
    session: Option<u64>,
}

impl Topic {
    fn scoped(kind: TopicKind, repo: &RepoKey) -> Self {
        Self {
            kind,
            scope: Some(repo.clone()),
            session: None,
        }
    }

    fn unique(id: u64) -> Self {
        Self {
            kind: TopicKind::Unique(id),
            scope: None,
            session: None,
        }
    }

    /// The repository this concerns, if any.
    ///
    /// Lets the UI attribute progress and errors to the tab they belong to
    /// rather than to whichever one happens to be showing.
    pub fn repo(&self) -> Option<&RepoKey> {
        self.scope.as_ref()
    }

    /// The tab this concerns, for work named by tab rather than repository.
    pub fn session(&self) -> Option<u64> {
        self.session
    }

    /// Whether a newer job on this topic invalidates an older one.
    fn is_supersedable(&self) -> bool {
        !matches!(self.kind, TopicKind::Unique(_))
    }
}

/// Work to perform off the UI thread.
#[derive(Debug, Clone)]
pub enum Job {
    /// Find the repository containing this path and read its head state.
    ///
    /// `token` identifies the session that asked, so the answer reaches the
    /// right tab even when several opens are in flight.
    OpenRepo { path: PathBuf, token: u64 },
    /// Re-read HEAD (branch, upstream, ahead/behind, in-progress operation).
    ReadHead(RepoKey),
    /// Re-read the working tree status.
    ReadStatus {
        repo: RepoKey,
        include_ignored: bool,
    },
    /// Walk history and assign graph lanes, up to `limit` commits.
    LoadGraph { repo: RepoKey, limit: usize },
    /// Read branches, remotes, tags, and stashes.
    LoadRefs(RepoKey),
    /// Compute a diff, highlighted for the given theme.
    LoadDiff {
        repo: RepoKey,
        target: DiffTarget,
        theme: HighlightTheme,
    },
    /// Attribute each line of a file to the commit that last changed it.
    Blame {
        repo: RepoKey,
        path: String,
        at: Option<git2::Oid>,
        theme: HighlightTheme,
    },
    /// Read the conflicted paths in the index.
    LoadConflicts(RepoKey),
    /// Read the repository's submodules and their states.
    LoadSubmodules(RepoKey),
    /// Read `user.name` and `user.email` at every level.
    ///
    /// `repo` is optional because the global identity belongs to the user
    /// rather than to any repository, and the settings sheet opens with or
    /// without one.
    ReadIdentity { repo: Option<RepoKey> },
    /// Write an identity to the global or repository config.
    SetIdentity {
        repo: Option<RepoKey>,
        scope: crate::git::identity::Scope,
        identity: crate::git::identity::Identity,
    },
    /// Search history.
    Search {
        repo: RepoKey,
        kind: SearchKind,
        query: String,
        limit: usize,
    },
    /// Commits touching one path, following renames.
    FileHistory {
        repo: RepoKey,
        path: String,
        limit: usize,
    },
    /// Change the index or the working tree.
    Mutate { repo: RepoKey, action: Mutation },
    /// Clone a repository. Unlike every other job this has no `RepoKey`,
    /// because the repository does not exist yet.
    Clone {
        url: String,
        parent: PathBuf,
        name: String,
    },
}

/// A change to the repository. Every variant runs on the single write worker,
/// so two of these can never interleave on the index.
#[derive(Debug, Clone)]
pub enum Mutation {
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
    /// Revert tracked files to their staged content.
    DiscardFiles(Vec<String>),
    /// Remove untracked files from disk. Not undoable, so the UI confirms.
    DeleteUntracked(Vec<String>),
    StageAll,
    /// Apply part of a file. `model` is carried by `Arc`, so this is cheap.
    Partial {
        model: Arc<DiffModel>,
        path: String,
        selections: Vec<HunkSelection>,
        kind: PartialKind,
    },
    Commit {
        message: String,
        mode: CommitMode,
    },

    // --- refs ---
    Checkout(String),
    CheckoutRemote(String),
    CheckoutCommit(git2::Oid),
    CreateBranch {
        name: String,
        start_point: Option<String>,
        checkout: bool,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    SetUpstream {
        branch: String,
        upstream: Option<String>,
    },
    CreateTag {
        name: String,
        target: git2::Oid,
        message: Option<String>,
    },
    DeleteTag(String),

    // --- stash ---
    StashSave {
        message: Option<String>,
        include_untracked: bool,
    },
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),

    // --- network ---
    Fetch {
        remote: Option<String>,
        prune: bool,
    },
    Pull(crate::git::remote::PullMode),
    Push {
        remote: String,
        branch: String,
        set_upstream: bool,
        mode: crate::git::remote::PushMode,
    },
    PushTag {
        remote: String,
        tag: String,
    },
    AddRemote {
        name: String,
        url: String,
    },
    RemoveRemote(String),

    // --- merge machinery ---
    Merge(String),
    CherryPick {
        oid: git2::Oid,
        /// 0 for an ordinary commit; the 1-based parent number for a merge.
        mainline: crate::git::merge::Mainline,
    },
    Revert {
        oid: git2::Oid,
        mainline: crate::git::merge::Mainline,
    },
    Reset {
        oid: git2::Oid,
        kind: ResetKind,
    },
    /// Abandon the merge, revert, or cherry-pick in progress.
    AbortOperation,
    ResolveConflict {
        path: String,
        resolution: Resolution,
    },
    ResolveConflictContent {
        path: String,
        content: String,
    },
    RebaseOnto(String),
    RebaseInteractive(Box<RebasePlan>),
    RebaseContinue,
    RebaseSkip,
    RebaseAbort,

    // --- submodules ---
    /// Clone or reset a submodule to the commit this repository records.
    /// `None` updates all of them.
    UpdateSubmodule(Option<String>),
    AddSubmodule {
        url: String,
        path: String,
    },
    RemoveSubmodule(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialKind {
    Stage,
    Unstage,
    Discard,
}

impl Mutation {
    /// Past-tense description, shown once the change lands.
    pub fn success_message(&self) -> String {
        let files = |paths: &Vec<String>| match paths.len() {
            1 => paths[0].rsplit('/').next().unwrap_or(&paths[0]).to_owned(),
            n => format!("{n} files"),
        };
        match self {
            Self::StageFiles(p) => format!("Staged {}", files(p)),
            Self::UnstageFiles(p) => format!("Unstaged {}", files(p)),
            Self::DiscardFiles(p) => format!("Discarded changes to {}", files(p)),
            Self::DeleteUntracked(p) => format!("Deleted {}", files(p)),
            Self::StageAll => "Staged all changes".to_owned(),
            Self::Partial { kind, .. } => match kind {
                PartialKind::Stage => "Staged selection".to_owned(),
                PartialKind::Unstage => "Unstaged selection".to_owned(),
                PartialKind::Discard => "Discarded selection".to_owned(),
            },
            Self::Commit { mode, .. } => match mode {
                CommitMode::Amend => "Amended the last commit".to_owned(),
                CommitMode::Normal => "Committed".to_owned(),
            },

            Self::Checkout(name) => format!("Switched to ‘{name}’"),
            Self::CheckoutRemote(name) => format!("Checked out ‘{name}’"),
            Self::CheckoutCommit(oid) => {
                format!("Detached HEAD at {}", crate::git::repo::short_id(*oid))
            }
            Self::CreateBranch { name, .. } => format!("Created ‘{name}’"),
            Self::DeleteBranch { name, .. } => format!("Deleted ‘{name}’"),
            Self::RenameBranch { new, .. } => format!("Renamed to ‘{new}’"),
            Self::SetUpstream { branch, upstream } => match upstream {
                Some(u) => format!("‘{branch}’ now tracks ‘{u}’"),
                None => format!("‘{branch}’ no longer tracks a remote"),
            },
            Self::CreateTag { name, .. } => format!("Tagged ‘{name}’"),
            Self::DeleteTag(name) => format!("Deleted tag ‘{name}’"),

            Self::StashSave { .. } => "Stashed your changes".to_owned(),
            Self::StashApply(_) => "Applied the stash".to_owned(),
            Self::StashPop(_) => "Popped the stash".to_owned(),
            Self::StashDrop(_) => "Dropped the stash entry".to_owned(),

            // Network results carry git's own summary, filled in by the worker.
            Self::Fetch { .. } | Self::Pull(_) | Self::Push { .. } | Self::PushTag { .. } => {
                String::new()
            }
            Self::AddRemote { name, .. } => format!("Added remote ‘{name}’"),
            Self::RemoveRemote(name) => format!("Removed remote ‘{name}’"),

            // Merge, cherry-pick, and revert report what actually happened,
            // which depends on whether they conflicted.
            Self::Merge(_)
            | Self::CherryPick { .. }
            | Self::Revert { .. }
            | Self::RebaseOnto(_)
            | Self::RebaseInteractive(_)
            | Self::RebaseContinue
            | Self::RebaseSkip => String::new(),
            Self::Reset { oid, kind } => format!(
                "{} reset to {}",
                kind.label(),
                crate::git::repo::short_id(*oid)
            ),
            Self::AbortOperation => "Aborted".to_owned(),
            Self::RebaseAbort => "Rebase aborted".to_owned(),
            Self::ResolveConflict { path, .. } | Self::ResolveConflictContent { path, .. } => {
                format!("Resolved {}", path.rsplit('/').next().unwrap_or(path))
            }
            // These report git's own summary.
            Self::UpdateSubmodule(_) | Self::AddSubmodule { .. } => String::new(),
            // Reports git's own summary, which says a commit is still needed.
            Self::RemoveSubmodule(_) => String::new(),
        }
    }

    /// Whether this changes which commit HEAD points at, so the graph and the
    /// diff cache both need rebuilding rather than just the status.
    pub fn moves_head(&self) -> bool {
        matches!(
            self,
            Self::Commit { .. }
                | Self::Checkout(_)
                | Self::CheckoutRemote(_)
                | Self::CheckoutCommit(_)
                | Self::CreateBranch { .. }
                | Self::DeleteBranch { .. }
                | Self::RenameBranch { .. }
                | Self::CreateTag { .. }
                | Self::DeleteTag(_)
                | Self::StashSave { .. }
                | Self::StashApply(_)
                | Self::StashPop(_)
                | Self::StashDrop(_)
                | Self::Fetch { .. }
                | Self::Pull(_)
                | Self::SetUpstream { .. }
                | Self::Merge(_)
                | Self::CherryPick { .. }
                | Self::Revert { .. }
                | Self::Reset { .. }
                | Self::AbortOperation
                | Self::RebaseOnto(_)
                | Self::RebaseInteractive(_)
                | Self::RebaseContinue
                | Self::RebaseSkip
                | Self::RebaseAbort
                | Self::UpdateSubmodule(_)
                | Self::AddSubmodule { .. }
                | Self::RemoveSubmodule(_)
        )
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "Committing",
            Self::DiscardFiles(_) | Self::DeleteUntracked(_) => "Discarding",
            Self::Checkout(_) | Self::CheckoutRemote(_) | Self::CheckoutCommit(_) => "Checking out",
            Self::CreateBranch { .. }
            | Self::DeleteBranch { .. }
            | Self::RenameBranch { .. }
            | Self::SetUpstream { .. } => "Updating branches",
            Self::CreateTag { .. } | Self::DeleteTag(_) => "Updating tags",
            Self::StashSave { .. }
            | Self::StashApply(_)
            | Self::StashPop(_)
            | Self::StashDrop(_) => "Stashing",
            Self::Fetch { .. } => "Fetching",
            Self::Pull(_) => "Pulling",
            Self::Push { .. } | Self::PushTag { .. } => "Pushing",
            Self::AddRemote { .. } | Self::RemoveRemote(_) => "Updating remotes",
            Self::Merge(_) => "Merging",
            Self::CherryPick { .. } => "Cherry-picking",
            Self::Revert { .. } => "Reverting",
            Self::Reset { .. } => "Resetting",
            Self::AbortOperation | Self::RebaseAbort => "Aborting",
            Self::ResolveConflict { .. } | Self::ResolveConflictContent { .. } => "Resolving",
            Self::RebaseOnto(_)
            | Self::RebaseInteractive(_)
            | Self::RebaseContinue
            | Self::RebaseSkip => "Rebasing",
            Self::UpdateSubmodule(_) | Self::AddSubmodule { .. } | Self::RemoveSubmodule(_) => {
                "Updating submodules"
            }
            _ => "Staging",
        }
    }
}

impl Job {
    fn topic(&self, unique: u64) -> Topic {
        use TopicKind as K;
        match self {
            // A clone has no repository yet, and an open is answering the
            // question of which repository it even is.
            Self::OpenRepo { token, .. } => Topic {
                kind: TopicKind::Unique(unique),
                scope: None,
                session: Some(*token),
            },
            Self::Clone { .. } => Topic::unique(unique),
            Self::ReadHead(repo) => Topic::scoped(K::Head, repo),
            Self::ReadStatus { repo, .. } => Topic::scoped(K::Status, repo),
            Self::LoadGraph { repo, .. } => Topic::scoped(K::Graph, repo),
            Self::LoadRefs(repo) => Topic::scoped(K::Refs, repo),
            Self::Blame { repo, .. } => Topic::scoped(K::Blame, repo),
            Self::LoadConflicts(repo) => Topic::scoped(K::Conflicts, repo),
            Self::LoadSubmodules(repo) => Topic::scoped(K::Submodules, repo),
            Self::ReadIdentity { repo } => Topic {
                kind: K::Identity,
                scope: repo.clone(),
                session: None,
            },
            // Never superseded: a half-written config is not a thing to be
            // cancelled because the user typed again.
            Self::SetIdentity { repo, .. } => Topic {
                kind: K::Unique(unique),
                scope: repo.clone(),
                session: None,
            },
            // File history shares the search topic: they render into the same
            // list, so one replacing the other is correct.
            Self::Search { repo, .. } | Self::FileHistory { repo, .. } => {
                Topic::scoped(K::Search, repo)
            }
            Self::LoadDiff { repo, target, .. } => {
                Topic::scoped(K::Diff(DiffSlot::of(*target)), repo)
            }
            // Mutations must never be superseded — a half-applied stage that
            // gets cancelled because the user clicked elsewhere is corruption —
            // but they still name their repository, so their progress and
            // failures can be attributed to the right tab.
            Self::Mutate { repo, .. } => Topic {
                kind: TopicKind::Unique(unique),
                scope: Some(repo.clone()),
                session: None,
            },
        }
    }

    /// Whether this job writes to the repository. Writes are serialized onto a
    /// single thread so two mutations can never race on the index.
    fn is_mutation(&self) -> bool {
        match self {
            Self::OpenRepo { .. }
            | Self::ReadHead(_)
            | Self::LoadGraph { .. }
            | Self::LoadRefs(_)
            | Self::LoadDiff { .. }
            | Self::Blame { .. }
            | Self::LoadConflicts(_)
            | Self::LoadSubmodules(_)
            | Self::Search { .. }
            | Self::FileHistory { .. }
            | Self::ReadIdentity { .. } => false,
            Self::Mutate { .. } | Self::Clone { .. } | Self::SetIdentity { .. } => true,
            // `update_index(true)` lets a status scan rewrite stat caches, so it
            // is a write as far as serialization is concerned.
            Self::ReadStatus { .. } => false,
        }
    }

    /// Shown in the status bar while the job runs.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::OpenRepo { .. } => "Opening repository",
            Self::ReadHead(_) => "Reading HEAD",
            Self::ReadStatus { .. } => "Reading status",
            Self::LoadGraph { .. } => "Loading history",
            Self::LoadRefs(_) => "Reading branches",
            Self::Blame { .. } => "Computing blame",
            Self::LoadConflicts(_) => "Reading conflicts",
            Self::LoadSubmodules(_) => "Reading submodules",
            Self::ReadIdentity { .. } => "Reading identity",
            Self::SetIdentity { .. } => "Saving identity",
            Self::Search { .. } => "Searching",
            Self::FileHistory { .. } => "Reading file history",
            Self::LoadDiff { .. } => "Computing diff",
            Self::Mutate { action, .. } => action.describe(),
            Self::Clone { .. } => "Cloning",
        }
    }
}

/// A successful result.
#[derive(Debug)]
pub enum Outcome {
    RepoOpened {
        /// The session that asked for this.
        token: u64,
        key: RepoKey,
        head: Box<HeadInfo>,
        /// The `.git` directory, which is not always inside the worktree.
        /// The filesystem watcher needs it separately.
        git_dir: PathBuf,
    },
    Head {
        key: RepoKey,
        head: Box<HeadInfo>,
    },
    Status {
        key: RepoKey,
        status: Arc<StatusSnapshot>,
    },
    Refs {
        key: RepoKey,
        tree: Arc<RefTree>,
    },
    Diff {
        key: RepoKey,
        target: DiffTarget,
        model: Arc<DiffModel>,
    },
    Blame {
        key: RepoKey,
        result: Arc<BlameResult>,
    },
    Conflicts {
        key: RepoKey,
        conflicts: Arc<Conflicts>,
    },
    Submodules {
        key: RepoKey,
        submodules: Arc<Submodules>,
    },
    Identity {
        key: Option<RepoKey>,
        identities: Box<crate::git::identity::Identities>,
    },
    Search {
        key: RepoKey,
        results: Arc<SearchResults>,
        kind: SearchKind,
    },
    FileHistory {
        key: RepoKey,
        results: Arc<SearchResults>,
        path: String,
    },
    Cloned {
        path: PathBuf,
    },
    /// The repository changed. Carries what to tell the user, and whether the
    /// history itself moved (so the graph needs rebuilding, not just the status).
    Mutated {
        key: RepoKey,
        message: String,
        moved_head: bool,
    },
    Graph {
        key: RepoKey,
        page: Arc<GraphPage>,
        /// The limit the walk was run with, so the UI knows whether a
        /// "load more" request has already been satisfied.
        limit: usize,
    },
}

/// Incremental progress for jobs slow enough that the user needs to see motion.
#[derive(Debug, Clone)]
pub struct Progress {
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(t) if t > 0 => Some((self.done as f32 / t as f32).clamp(0.0, 1.0)),
            _ => None,
        }
    }
}

/// What a worker sends back.
#[derive(Debug)]
pub enum Message {
    Done {
        id: u64,
        topic: Topic,
        outcome: Outcome,
    },
    Failed {
        id: u64,
        topic: Topic,
        error: Error,
    },
    Progress {
        id: u64,
        topic: Topic,
        progress: Progress,
    },
}

impl Message {
    fn id(&self) -> u64 {
        match self {
            Self::Done { id, .. } | Self::Failed { id, .. } | Self::Progress { id, .. } => *id,
        }
    }

    fn topic(&self) -> &Topic {
        match self {
            Self::Done { topic, .. }
            | Self::Failed { topic, .. }
            | Self::Progress { topic, .. } => topic,
        }
    }
}

/// Cooperative cancellation. Long operations poll this and bail out early;
/// short ones are simply discarded on arrival.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Ask the job to stop. Public so the UI can offer a cancel button for
    /// long network operations.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Convenience for the `?` operator inside worker code.
    pub fn check(&self) -> crate::error::Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// One unit of work as handed to a worker.
pub(crate) struct Task {
    pub id: u64,
    pub topic: Topic,
    pub job: Job,
    pub cancel: Cancel,
}

/// Handle used by the UI thread. Not `Sync` on purpose — it belongs to the UI.
pub struct JobSystem {
    read_tx: Sender<Task>,
    write_tx: Sender<Task>,
    rx: Receiver<Message>,
    next_id: u64,
    /// Newest job id dispatched per supersedable topic.
    current: HashMap<Topic, u64>,
    /// Cancel handles for jobs still in flight, so a newer job can stop them.
    inflight: HashMap<u64, (Topic, Cancel, &'static str)>,
    workers: Vec<JoinHandle<()>>,
}

impl JobSystem {
    /// Spawn the worker pool.
    ///
    /// Reads get several threads because they are the common case and are
    /// independent. Writes get exactly one, which is what makes index
    /// mutations safe without any locking in the Git layer itself.
    pub fn new(ctx: egui::Context) -> Self {
        let read_threads = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).clamp(2, 6))
            .unwrap_or(3);

        let (read_tx, read_rx) = crossbeam_channel::unbounded::<Task>();
        let (write_tx, write_rx) = crossbeam_channel::unbounded::<Task>();
        let (msg_tx, rx) = crossbeam_channel::unbounded::<Message>();

        let mut workers = Vec::with_capacity(read_threads + 1);
        for i in 0..read_threads {
            workers.push(worker::spawn(
                format!("gitup-read-{i}"),
                read_rx.clone(),
                msg_tx.clone(),
                ctx.clone(),
            ));
        }
        workers.push(worker::spawn(
            "gitup-write".to_owned(),
            write_rx,
            msg_tx,
            ctx,
        ));

        Self {
            read_tx,
            write_tx,
            rx,
            next_id: 1,
            current: HashMap::new(),
            inflight: HashMap::new(),
            workers,
        }
    }

    /// Queue a job, superseding any older job on the same topic.
    pub fn dispatch(&mut self, job: Job) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let topic = job.topic(id);

        if topic.is_supersedable() {
            // Signal the previous holder of this topic to stop. It may already
            // be finished, in which case this is a no-op and its message gets
            // filtered by `poll`.
            self.inflight.retain(|_, (t, cancel, _)| {
                if *t == topic {
                    cancel.cancel();
                    false
                } else {
                    true
                }
            });
            self.current.insert(topic.clone(), id);
        }

        let cancel = Cancel::default();
        self.inflight
            .insert(id, (topic.clone(), cancel.clone(), job.describe()));

        let mutation = job.is_mutation();
        let task = Task {
            id,
            topic,
            job,
            cancel,
        };
        let tx = if mutation {
            &self.write_tx
        } else {
            &self.read_tx
        };
        // A send failure means every worker is gone, which only happens during
        // shutdown; dropping the task then is correct.
        let _ = tx.send(task);
        id
    }

    /// Collect finished work. Stale results are dropped here, so callers only
    /// ever see messages that still matter.
    pub fn poll(&mut self) -> Vec<Message> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            let id = msg.id();

            let is_terminal = !matches!(msg, Message::Progress { .. });
            if is_terminal {
                self.inflight.remove(&id);
            }

            // Drop anything a newer job on the same topic has replaced.
            let superseded = {
                let topic = msg.topic();
                topic.is_supersedable()
                    && self.current.get(topic).is_some_and(|current| id < *current)
            };
            if superseded {
                continue;
            }

            if let Message::Failed { error, .. } = &msg {
                if error.is_cancelled() {
                    continue;
                }
            }

            out.push(msg);
        }
        out
    }

    /// Whether an identical request is already in flight.
    ///
    /// Callers ask the job system rather than remembering for themselves. A
    /// caller-side record cannot know when its request was superseded — the
    /// result it is waiting for simply never arrives — so it stays marked
    /// outstanding forever, and the next time that exact work is wanted it is
    /// skipped as already asked for.
    ///
    /// Jobs that are never superseded always report `false`: a mutation must
    /// always be dispatched, even if an identical one is running.
    pub fn is_pending(&self, job: &Job) -> bool {
        // The sentinel only affects `Unique` topics, which are excluded below.
        let topic = job.topic(u64::MAX);
        topic.is_supersedable()
            && self
                .inflight
                .values()
                .any(|(pending, _, _)| *pending == topic)
    }

    /// True while any job is running — drives the activity indicator.
    pub fn is_busy(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// Labels of everything currently running, for the status bar.
    pub fn active_labels(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.inflight.values().map(|(_, _, label)| *label).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}
