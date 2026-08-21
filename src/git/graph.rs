//! Commit history and graph lane assignment.
//!
//! Lanes are computed here, on a worker thread, not in the renderer. Each row
//! carries exactly what the painter needs — which lanes pass through, which
//! converge into the commit, which leave it — so drawing a row is a handful of
//! line segments with no lookahead and no shared state. That is what lets the
//! view virtualize: row 40 000 can be drawn without having drawn row 39 999.

use crate::error::Result;
use git2::{Oid, Repository, Sort};
use std::collections::HashMap;
use std::sync::Arc;

/// One commit, flattened for display.
#[derive(Debug, Clone)]
pub struct CommitSummary {
    pub id: Oid,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    /// The author's UTC offset, in minutes.
    pub tz_offset_minutes: i32,
    pub parents: Vec<Oid>,
}

impl CommitSummary {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

/// A branch or tag label drawn next to the commit it points at.
#[derive(Debug, Clone)]
pub struct RefBadge {
    pub name: String,
    pub kind: RefKind,
    /// True when this is the branch HEAD is currently on.
    pub is_head: bool,
}

/// A line segment in a row's vertical strip, identified by lane index. The
/// colour is derived from the lane, so lanes keep their colour for as long as
/// they are alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub lane: usize,
}

#[derive(Debug, Clone)]
pub struct GraphRow {
    pub commit: CommitSummary,
    /// The lane the commit's dot sits in.
    pub lane: usize,
    /// Lanes crossing this row without touching the commit: drawn top to bottom.
    pub passthrough: Vec<Segment>,
    /// Lanes arriving from above and converging into this commit.
    pub incoming: Vec<Segment>,
    /// Lanes leaving this commit downward. The first parent continues in the
    /// commit's own lane; further parents branch off into their own.
    pub outgoing: Vec<Segment>,
    /// Highest lane index in use at this row, so the view can size the gutter.
    pub width: usize,
    pub refs: Vec<RefBadge>,
}

#[derive(Debug, Clone, Default)]
pub struct GraphPage {
    pub rows: Vec<GraphRow>,
    /// True when the walk stopped at the limit rather than at the end of history.
    pub has_more: bool,
    /// Widest lane count anywhere in the page, for a stable gutter width.
    pub max_width: usize,
}

/// Lane bookkeeping.
///
/// `slots[i] == Some(oid)` means lane `i` currently has a line descending from
/// above that is waiting for commit `oid`. `None` is a free lane, reusable by
/// the next branch that needs one — which is what keeps the graph narrow
/// instead of drifting endlessly rightward.
#[derive(Default)]
struct Lanes {
    slots: Vec<Option<Oid>>,
}

impl Lanes {
    /// Lane currently waiting for `oid`, if any.
    fn find(&self, oid: Oid) -> Option<usize> {
        self.slots.iter().position(|s| *s == Some(oid))
    }

    /// Every lane waiting for `oid`. More than one means several children share
    /// this parent, and their lines converge here.
    fn find_all(&self, oid: Oid) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (*s == Some(oid)).then_some(i))
            .collect()
    }

    /// Claim the leftmost free lane, extending the row if all are taken.
    fn allocate(&mut self, oid: Oid) -> usize {
        match self.slots.iter().position(|s| s.is_none()) {
            Some(i) => {
                self.slots[i] = Some(oid);
                i
            }
            None => {
                self.slots.push(Some(oid));
                self.slots.len() - 1
            }
        }
    }

    fn set(&mut self, lane: usize, oid: Option<Oid>) {
        if lane < self.slots.len() {
            self.slots[lane] = oid;
        }
    }

    /// Number of lanes in use, ignoring trailing free slots.
    fn width(&self) -> usize {
        self.slots
            .iter()
            .rposition(|s| s.is_some())
            .map_or(0, |i| i + 1)
    }

    /// Drop trailing free lanes so the gutter shrinks once branches merge back.
    fn compact(&mut self) {
        while matches!(self.slots.last(), Some(None)) {
            self.slots.pop();
        }
    }
}

/// Walk history and assign lanes.
///
/// `limit` caps how many rows come back. It is worth being precise about what
/// that does and does not buy, because it is easy to assume otherwise:
///
/// libgit2 resolves a topologically sorted revwalk *eagerly* — the first call
/// to `next()` traverses the whole reachable graph, and every subsequent one is
/// nearly free. Measured on a ten-thousand-commit history, that first step
/// costs about 250ms while pulling the remaining 9 999 commits costs 0.2ms, and
/// turning all ten thousand into summaries costs another 0.7ms.
///
/// So `limit` bounds **memory**, not time: it stops a million-commit repository
/// from materializing a million `CommitSummary` values. The traversal happens
/// regardless, which is why this runs on a worker while the view shows a
/// skeleton, and why the initial limit is set high enough that most
/// repositories are walked once rather than again on every scroll.
pub fn build(
    repo: &Repository,
    limit: usize,
    cancel: &crate::job::Cancel,
) -> Result<Arc<GraphPage>> {
    // The walk cannot be interrupted once it starts, so check before paying for
    // it rather than only inside the loop below.
    cancel.check()?;

    let decorations = collect_refs(repo)?;

    let mut walk = repo.revwalk()?;
    // Topological order keeps a branch's commits contiguous instead of
    // interleaving them by date, which is what makes the lanes readable — and
    // it guarantees a commit is seen after all of its children, which the lane
    // assignment depends on. Time order breaks ties so the result still reads
    // newest-first.
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

    // Push every branch, not just HEAD: a client that can only show the current
    // branch is not showing you the repository.
    let mut pushed = false;
    for glob in ["refs/heads/*", "refs/remotes/*", "refs/tags/*"] {
        if walk.push_glob(glob).is_ok() {
            pushed = true;
        }
    }
    // Only needed when HEAD is detached; otherwise `refs/heads/*` already
    // covered it, and pushing the same tip twice is redundant work.
    if (!pushed || repo.head_detached().unwrap_or(false)) && walk.push_head().is_ok() {
        pushed = true;
    }
    if !pushed {
        // An unborn HEAD with no refs at all: an empty graph, not an error.
        return Ok(Arc::new(GraphPage::default()));
    }

    let mut lanes = Lanes::default();
    let mut rows: Vec<GraphRow> = Vec::new();
    let mut max_width = 0usize;
    let mut has_more = false;

    for (i, oid) in walk.enumerate() {
        if i >= limit {
            has_more = true;
            break;
        }
        // Cancellation matters here: this loop can run for a long time, and the
        // user may have already asked for something else.
        if i % 512 == 0 {
            cancel.check()?;
        }

        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let summary = summarize(&commit);

        // Lines arriving from above that were waiting for this commit.
        let arriving = lanes.find_all(oid);
        let lane = match arriving.first() {
            Some(&first) => first,
            // No child has been seen yet, so this is a tip: it needs a new lane.
            None => lanes.allocate(oid),
        };

        // Extra arrivals converge into `lane` and release their own.
        for &extra in arriving.iter().skip(1) {
            lanes.set(extra, None);
        }

        let incoming: Vec<Segment> = arriving.iter().map(|&l| Segment { lane: l }).collect();

        // The commit's own lane continues with its first parent, so a linear
        // history stays in one straight column.
        //
        // Two lanes are allowed to expect the same parent. That looks like
        // duplication but it is exactly what produces the `|/` convergence in
        // `git log --graph`: both lines keep descending in their own lanes and
        // meet at the shared parent, which claims the leftmost of them. Bending
        // one lane into the other here instead would drag the trunk sideways
        // whenever a side branch happened to be committed more recently.
        let mut outgoing = Vec::new();
        match summary.parents.first() {
            Some(&first_parent) => {
                lanes.set(lane, Some(first_parent));
                outgoing.push(Segment { lane });
            }
            None => lanes.set(lane, None),
        }

        // Merge parents branch off into their own lanes.
        for &parent in summary.parents.iter().skip(1) {
            let target = lanes.find(parent).unwrap_or_else(|| lanes.allocate(parent));
            outgoing.push(Segment { lane: target });
        }

        // Anything still active and not touching this commit crosses the row.
        let touched: Vec<usize> = incoming
            .iter()
            .chain(outgoing.iter())
            .map(|s| s.lane)
            .chain(std::iter::once(lane))
            .collect();
        let passthrough: Vec<Segment> = lanes
            .slots
            .iter()
            .enumerate()
            .filter_map(|(l, slot)| {
                (slot.is_some() && !touched.contains(&l)).then_some(Segment { lane: l })
            })
            .collect();

        lanes.compact();
        let width = lanes.width().max(lane + 1);
        max_width = max_width.max(width);

        rows.push(GraphRow {
            refs: decorations.get(&oid).cloned().unwrap_or_default(),
            commit: summary,
            lane,
            passthrough,
            incoming,
            outgoing,
            width,
        });
    }

    Ok(Arc::new(GraphPage {
        rows,
        has_more,
        max_width,
    }))
}

/// Flatten a commit for display. Shared with blame and search, which need
/// the same fields without walking the graph.
pub fn summarize(commit: &git2::Commit<'_>) -> CommitSummary {
    let author = commit.author();
    let time = commit.time();
    CommitSummary {
        id: commit.id(),
        short_id: super::repo::short_id(commit.id()),
        summary: commit
            .summary()
            .ok()
            .flatten()
            .unwrap_or("(no message)")
            .to_owned(),
        author_name: author.name().unwrap_or("(unknown)").to_owned(),
        author_email: author.email().unwrap_or_default().to_owned(),
        time: time.seconds(),
        tz_offset_minutes: time.offset_minutes(),
        parents: commit.parent_ids().collect(),
    }
}

/// Map every commit that a ref points at to its labels.
fn collect_refs(repo: &Repository) -> Result<HashMap<Oid, Vec<RefBadge>>> {
    let mut map: HashMap<Oid, Vec<RefBadge>> = HashMap::new();

    let head_target = repo.head().ok().and_then(|h| {
        if h.is_branch() {
            h.shorthand().ok().map(str::to_owned)
        } else {
            None
        }
    });

    for reference in repo.references()?.flatten() {
        let Ok(name) = reference.name() else { continue };

        // Tags need peeling: an annotated tag points at a tag object, not the
        // commit, and labelling the tag object would put the badge nowhere.
        let Some(oid) = reference
            .peel_to_commit()
            .ok()
            .map(|c| c.id())
            .or_else(|| reference.target())
        else {
            continue;
        };

        let (kind, short) = if let Some(rest) = name.strip_prefix("refs/heads/") {
            (RefKind::LocalBranch, rest.to_owned())
        } else if let Some(rest) = name.strip_prefix("refs/remotes/") {
            // `origin/HEAD` is a symbolic pointer, not a branch worth labelling.
            if rest.ends_with("/HEAD") {
                continue;
            }
            (RefKind::RemoteBranch, rest.to_owned())
        } else if let Some(rest) = name.strip_prefix("refs/tags/") {
            (RefKind::Tag, rest.to_owned())
        } else {
            continue;
        };

        let is_head = kind == RefKind::LocalBranch && head_target.as_deref() == Some(&short);
        map.entry(oid).or_default().push(RefBadge {
            name: short,
            kind,
            is_head,
        });
    }

    // A detached HEAD gets its own badge, since no branch names that commit.
    if head_target.is_none() {
        if let Ok(oid) = repo.head().and_then(|h| h.peel_to_commit()).map(|c| c.id()) {
            map.entry(oid).or_default().push(RefBadge {
                name: "HEAD".to_owned(),
                kind: RefKind::Head,
                is_head: true,
            });
        }
    }

    // Order badges so the checked-out branch reads first, then locals, remotes,
    // tags — and alphabetically within each group, so the list is stable.
    for badges in map.values_mut() {
        badges.sort_by(|a, b| {
            let rank = |r: &RefBadge| match (r.is_head, r.kind) {
                (true, _) => 0,
                (_, RefKind::Head) => 1,
                (_, RefKind::LocalBranch) => 2,
                (_, RefKind::RemoteBranch) => 3,
                (_, RefKind::Tag) => 4,
            };
            rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
        });
    }

    Ok(map)
}
