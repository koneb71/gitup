//! The reference tree: branches, remotes, tags, and stashes.

use crate::error::Result;
use git2::{BranchType, Oid, Repository};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct BranchEntry {
    /// Short name, e.g. `main` or `feature/parser`.
    pub name: String,
    pub target: Option<Oid>,
    /// True for the branch HEAD is on.
    pub is_head: bool,
    /// Configured upstream, e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone)]
pub struct RemoteBranchEntry {
    /// Name without the remote prefix, e.g. `main`.
    pub name: String,
    pub target: Option<Oid>,
}

#[derive(Debug, Clone)]
pub struct RemoteGroup {
    pub name: String,
    pub url: Option<String>,
    pub branches: Vec<RemoteBranchEntry>,
}

#[derive(Debug, Clone)]
pub struct TagEntry {
    pub name: String,
    pub target: Option<Oid>,
    /// Annotated tags carry a message; lightweight ones don't.
    pub annotated: bool,
}

#[derive(Debug, Clone)]
pub struct StashEntry {
    pub index: usize,
    pub message: String,
    pub target: Oid,
}

#[derive(Debug, Clone, Default)]
pub struct RefTree {
    pub local: Vec<BranchEntry>,
    pub remotes: Vec<RemoteGroup>,
    pub tags: Vec<TagEntry>,
    pub stashes: Vec<StashEntry>,
}

impl RefTree {
    pub fn head_branch(&self) -> Option<&BranchEntry> {
        self.local.iter().find(|b| b.is_head)
    }

    pub fn branch(&self, name: &str) -> Option<&BranchEntry> {
        self.local.iter().find(|b| b.name == name)
    }

    pub fn total_branches(&self) -> usize {
        self.local.len() + self.remotes.iter().map(|r| r.branches.len()).sum::<usize>()
    }
}

/// `stash_foreach` needs `&mut Repository`, which is why this takes one even
/// though everything else here only reads.
pub fn build(repo: &mut Repository) -> Result<Arc<RefTree>> {
    let mut tree = RefTree {
        local: local_branches(repo)?,
        remotes: remotes(repo)?,
        tags: tags(repo)?,
        stashes: Vec::new(),
    };

    // Stash entries live in a reflog, not the ref namespace, so they need their
    // own walk. A repository with no stash ref simply yields nothing.
    let mut stashes = Vec::new();
    let _ = repo.stash_foreach(|index, message, oid| {
        stashes.push(StashEntry {
            index,
            message: message.to_owned(),
            target: *oid,
        });
        true
    });
    tree.stashes = stashes;

    Ok(Arc::new(tree))
}

fn local_branches(repo: &Repository) -> Result<Vec<BranchEntry>> {
    let mut out = Vec::new();
    for item in repo.branches(Some(BranchType::Local))? {
        let (branch, _) = item?;
        let Ok(name) = branch.name() else { continue };
        let Some(name) = name else { continue };
        let name = name.to_owned();
        let target = branch.get().target();
        let is_head = branch.is_head();

        // Upstream tracking is optional and its absence is normal, so every
        // failure here degrades to "no upstream" rather than propagating.
        let (upstream, ahead, behind) = match branch.upstream() {
            Ok(up) => {
                let up_name = up.name().ok().flatten().map(str::to_owned);
                let counts = match (target, up.get().target()) {
                    (Some(local), Some(remote)) => {
                        repo.graph_ahead_behind(local, remote).unwrap_or((0, 0))
                    }
                    _ => (0, 0),
                };
                (up_name, counts.0, counts.1)
            }
            Err(_) => (None, 0, 0),
        };

        out.push(BranchEntry {
            name,
            target,
            is_head,
            upstream,
            ahead,
            behind,
        });
    }

    // Checked-out branch first, then alphabetical — the one you are on is the
    // one you look for.
    out.sort_by(|a, b| {
        b.is_head
            .cmp(&a.is_head)
            .then_with(|| natural_cmp(&a.name, &b.name))
    });
    Ok(out)
}

fn remotes(repo: &Repository) -> Result<Vec<RemoteGroup>> {
    // `StringArray` yields `Result<Option<&str>, _>`: an entry can be absent or
    // invalid UTF-8, and neither is worth failing the whole tree over.
    let remote_names: Vec<String> = repo
        .remotes()?
        .iter()
        .filter_map(|entry| entry.ok().flatten().map(str::to_owned))
        .collect();

    let mut groups: Vec<RemoteGroup> = remote_names
        .into_iter()
        .map(|name| {
            let url = repo
                .find_remote(&name)
                .ok()
                .and_then(|r| r.url().ok().map(str::to_owned));
            RemoteGroup {
                name,
                url,
                branches: Vec::new(),
            }
        })
        .collect();

    for item in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = item?;
        let Ok(Some(full)) = branch.name() else {
            continue;
        };
        // `origin/HEAD` is a symbolic pointer, not a branch.
        if full.ends_with("/HEAD") {
            continue;
        }
        let Some((remote, short)) = full.split_once('/') else {
            continue;
        };
        let entry = RemoteBranchEntry {
            name: short.to_owned(),
            target: branch.get().target(),
        };
        match groups.iter_mut().find(|g| g.name == remote) {
            Some(group) => group.branches.push(entry),
            // A remote-tracking branch whose remote was deleted still exists;
            // showing it under its own heading beats dropping it.
            None => groups.push(RemoteGroup {
                name: remote.to_owned(),
                url: None,
                branches: vec![entry],
            }),
        }
    }

    for group in &mut groups {
        group.branches.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    }
    groups.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    Ok(groups)
}

fn tags(repo: &Repository) -> Result<Vec<TagEntry>> {
    let mut out = Vec::new();
    for reference in repo.references_glob("refs/tags/*")?.flatten() {
        let Ok(name) = reference.name() else { continue };
        let Some(short) = name.strip_prefix("refs/tags/") else {
            continue;
        };
        let annotated = reference
            .target()
            .and_then(|oid| repo.find_tag(oid).ok())
            .is_some();
        out.push(TagEntry {
            name: short.to_owned(),
            target: reference.peel_to_commit().ok().map(|c| c.id()),
            annotated,
        });
    }
    // Newest-looking versions first: `v10` should not sort before `v9`.
    out.sort_by(|a, b| natural_cmp(&b.name, &a.name));
    Ok(out)
}

/// Compare strings with embedded numbers numerically.
///
/// Plain lexicographic order puts `v10` before `v9`, which is wrong in exactly
/// the place users look most: a list of version tags.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.char_indices().peekable();
    let mut bi = b.char_indices().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some((ap, ac)), Some((bp, bc))) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_num = take_number(a, ap);
                    let b_num = take_number(b, bp);
                    // Compare by value, then by length so `01` and `1` are
                    // ordered deterministically rather than declared equal.
                    let ord = a_num
                        .1
                        .cmp(&b_num.1)
                        .then_with(|| a_num.0.len().cmp(&b_num.0.len()));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    for _ in 0..a_num.0.chars().count() {
                        ai.next();
                    }
                    for _ in 0..b_num.0.chars().count() {
                        bi.next();
                    }
                } else {
                    let ord = ac
                        .to_ascii_lowercase()
                        .cmp(&bc.to_ascii_lowercase())
                        .then_with(|| ac.cmp(&bc));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

/// The digit run starting at `start`, and its numeric value (saturating).
fn take_number(s: &str, start: usize) -> (&str, u128) {
    let rest = &s[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let digits = &rest[..end];
    let value = digits.parse::<u128>().unwrap_or(u128::MAX);
    (digits, value)
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn numbers_sort_numerically() {
        assert_eq!(natural_cmp("v9", "v10"), Ordering::Less);
        assert_eq!(natural_cmp("v10", "v9"), Ordering::Greater);
        assert_eq!(natural_cmp("v1.2.10", "v1.2.9"), Ordering::Greater);
    }

    #[test]
    fn text_sorts_case_insensitively_but_deterministically() {
        assert_eq!(natural_cmp("alpha", "Beta"), Ordering::Less);
        assert_ne!(natural_cmp("a", "A"), Ordering::Equal);
    }

    #[test]
    fn prefixes_sort_before_longer_strings() {
        assert_eq!(natural_cmp("feat", "feature"), Ordering::Less);
        assert_eq!(natural_cmp("main", "main"), Ordering::Equal);
    }

    #[test]
    fn enormous_numbers_do_not_panic() {
        let huge = "v".to_owned() + &"9".repeat(60);
        assert_eq!(natural_cmp(&huge, &huge), Ordering::Equal);
    }
}
