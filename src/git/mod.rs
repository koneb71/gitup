//! The Git layer. Everything here runs on worker threads — never the UI thread.

pub mod blame;
pub mod branch;
pub mod cli;
pub mod commit;
pub mod conflict;
pub mod diff;
pub mod graph;
pub mod highlight;
pub mod identity;
pub mod inline;
pub mod lfs;
pub mod merge;
pub mod message;
pub mod rebase;
pub mod refs;
pub mod remote;
pub mod repo;
pub mod search;
pub mod stage;
pub mod stash;
pub mod status;
pub mod submodule;

pub use diff::{DiffLine, DiffModel, DiffTarget, FileDiff, Hunk, LineKind};
pub use graph::{CommitSummary, GraphPage, GraphRow, RefBadge, RefKind};
pub use refs::{BranchEntry, RefTree, RemoteGroup, StashEntry, TagEntry};
#[allow(unused_imports)]
pub use repo::{HeadInfo, HeadKind, PendingOp, RepoKey, UpstreamInfo};
pub use status::{Delta, StatusEntry, StatusSnapshot};
