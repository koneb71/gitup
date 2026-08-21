//! Staging against real repositories.
//!
//! The unit tests in `git::stage` check the shape of the generated patch; these
//! check that git actually accepts it and that the index ends up holding
//! exactly the content it should. That distinction matters: a patch can be
//! well-formed and still stage the wrong thing.

mod common;

use common::Fixture;
use git2::Repository;
use gitup::git::diff::{self, DiffTarget};
use gitup::git::highlight::HighlightTheme;
use gitup::git::stage::{self, HunkSelection};
use gitup::job::Cancel;
use std::path::Path;
use std::sync::Arc;

fn unstaged(f: &Fixture) -> Arc<diff::DiffModel> {
    diff::build(
        &f.repo,
        DiffTarget::Unstaged,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("unstaged diff")
}

fn staged(f: &Fixture) -> Arc<diff::DiffModel> {
    diff::build(
        &f.repo,
        DiffTarget::Staged,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("staged diff")
}

/// The content git has recorded in the index for a path.
fn index_content(repo: &Repository, path: &str) -> String {
    let index = repo.index().expect("index");
    let entry = index
        .get_path(Path::new(path), 0)
        .unwrap_or_else(|| panic!("{path} is not in the index"));
    let blob = repo.find_blob(entry.id).expect("blob");
    String::from_utf8_lossy(blob.content()).into_owned()
}

fn worktree_content(f: &Fixture, path: &str) -> String {
    std::fs::read_to_string(f.path().join(path)).expect("read worktree file")
}

/// A file with five numbered lines, committed, then edited in three places.
fn three_edits() -> Fixture {
    let f = Fixture::empty_named("staging");
    f.commit_file("f.txt", "one\ntwo\nthree\nfour\nfive\n", "Add file");
    f.write("f.txt", "ONE\ntwo\nTHREE\nfour\nFIVE\n");
    f
}

#[test]
fn staging_a_whole_file_records_the_worktree_content() {
    let f = three_edits();
    stage::stage_files(&f.repo, &["f.txt".to_owned()]).expect("stage");
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n"
    );
}

#[test]
fn staging_one_line_leaves_the_others_alone() {
    let f = three_edits();
    let model = unstaged(&f);
    let file = model.find("f.txt").expect("f.txt in the diff");

    // Find the addition of "THREE" and stage only it.
    let (hunk_index, line_index) = locate(file, "THREE");
    stage::stage_partial(
        &f.repo,
        file,
        &[HunkSelection {
            hunk_index,
            lines: Some(vec![line_index, locate(file, "three").1]),
        }],
    )
    .expect("stage one line");

    // The index has the third line changed and nothing else. This is the
    // assertion the whole feature exists for.
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "one\ntwo\nTHREE\nfour\nfive\n",
        "only the selected line should be staged"
    );
    // The working tree is untouched — staging never edits your files.
    assert_eq!(
        worktree_content(&f, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n"
    );
}

/// Index of the (hunk, line) whose content matches exactly.
fn locate(file: &diff::FileDiff, content: &str) -> (usize, usize) {
    for (h, hunk) in file.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            if line.content == content {
                return (h, l);
            }
        }
    }
    panic!(
        "no line {content:?}; have {:?}",
        file.hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| (l.kind, l.content.clone())))
            .collect::<Vec<_>>()
    )
}

#[test]
fn staging_then_unstaging_the_same_line_is_a_round_trip() {
    let f = three_edits();
    let model = unstaged(&f);
    let file = model.find("f.txt").expect("f.txt");
    let (hunk_index, _) = locate(file, "THREE");
    let selection = HunkSelection {
        hunk_index,
        lines: Some(vec![locate(file, "three").1, locate(file, "THREE").1]),
    };
    stage::stage_partial(&f.repo, file, &[selection]).expect("stage");
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "one\ntwo\nTHREE\nfour\nfive\n"
    );

    // Now unstage it, working from the *staged* diff.
    let staged_model = staged(&f);
    let staged_file = staged_model.find("f.txt").expect("f.txt staged");
    let (sh, _) = locate(staged_file, "THREE");
    stage::unstage_partial(
        &f.repo,
        staged_file,
        &[HunkSelection {
            hunk_index: sh,
            lines: Some(vec![
                locate(staged_file, "three").1,
                locate(staged_file, "THREE").1,
            ]),
        }],
    )
    .expect("unstage");

    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "one\ntwo\nthree\nfour\nfive\n",
        "unstaging should restore the committed content"
    );
}

#[test]
fn staging_a_whole_hunk_stages_every_change_in_it() {
    let f = Fixture::empty_named("hunk");
    // Two edits far enough apart to land in separate hunks.
    let original: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    f.commit_file("f.txt", &original, "Add");
    let edited: String = (1..=40)
        .map(|i| match i {
            3 => "line 3 EDITED\n".to_owned(),
            4 => "line 4 EDITED\n".to_owned(),
            30 => "line 30 EDITED\n".to_owned(),
            _ => format!("line {i}\n"),
        })
        .collect();
    f.write("f.txt", &edited);

    let model = unstaged(&f);
    let file = model.find("f.txt").expect("f.txt");
    assert!(
        file.hunks.len() >= 2,
        "expected the edits to be separate hunks"
    );

    stage::stage_partial(&f.repo, file, &[HunkSelection::whole(0)]).expect("stage hunk");

    let staged_content = index_content(&f.repo, "f.txt");
    assert!(staged_content.contains("line 3 EDITED"));
    assert!(staged_content.contains("line 4 EDITED"));
    assert!(
        !staged_content.contains("line 30 EDITED"),
        "the second hunk must be untouched"
    );
}

#[test]
fn discarding_one_line_edits_the_working_tree_only_there() {
    let f = three_edits();
    let model = unstaged(&f);
    let file = model.find("f.txt").expect("f.txt");
    let hunk_index = locate(file, "ONE").0;

    stage::discard_partial(
        &f.repo,
        file,
        &[HunkSelection {
            hunk_index,
            lines: Some(vec![locate(file, "one").1, locate(file, "ONE").1]),
        }],
    )
    .expect("discard");

    assert_eq!(
        worktree_content(&f, "f.txt"),
        "one\ntwo\nTHREE\nfour\nFIVE\n",
        "only the discarded line should revert"
    );
}

#[test]
fn discarding_one_line_when_other_edits_exist_matches_the_working_tree() {
    // The case that exposes the mirrored-context rule: the patch's context has
    // to describe the *working tree*, which still holds the other two edits.
    let f = three_edits();
    let model = unstaged(&f);
    let file = model.find("f.txt").expect("f.txt");
    let hunk_index = locate(file, "FIVE").0;

    stage::discard_partial(
        &f.repo,
        file,
        &[HunkSelection {
            hunk_index,
            lines: Some(vec![locate(file, "five").1, locate(file, "FIVE").1]),
        }],
    )
    .expect("discard the last edit");

    assert_eq!(
        worktree_content(&f, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nfive\n",
        "the other two edits must be left in place"
    );
}

#[test]
fn unstaging_one_line_when_other_edits_are_staged() {
    let f = three_edits();
    stage::stage_files(&f.repo, &["f.txt".to_owned()]).expect("stage everything");
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n"
    );

    let model = staged(&f);
    let file = model.find("f.txt").expect("f.txt staged");
    let hunk_index = locate(file, "THREE").0;

    stage::unstage_partial(
        &f.repo,
        file,
        &[HunkSelection {
            hunk_index,
            lines: Some(vec![locate(file, "three").1, locate(file, "THREE").1]),
        }],
    )
    .expect("unstage the middle edit");

    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "ONE\ntwo\nthree\nfour\nFIVE\n",
        "only the middle line should revert in the index"
    );
    assert_eq!(
        worktree_content(&f, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n",
        "the working tree is never touched by unstaging"
    );
}

#[test]
fn staging_a_new_file_by_patch_adds_it_to_the_index() {
    let f = Fixture::empty_named("untracked");
    f.commit_file("seed.txt", "seed\n", "Seed");
    f.write("fresh.txt", "alpha\nbeta\n");

    let model = unstaged(&f);
    let file = model.find("fresh.txt").expect("untracked file in the diff");
    stage::stage_partial(&f.repo, file, &[HunkSelection::whole(0)]).expect("stage new file");

    assert_eq!(index_content(&f.repo, "fresh.txt"), "alpha\nbeta\n");
}

#[test]
fn staging_part_of_a_new_file_records_only_those_lines() {
    let f = Fixture::empty_named("partial_new");
    f.commit_file("seed.txt", "seed\n", "Seed");
    f.write("fresh.txt", "alpha\nbeta\ngamma\n");

    let model = unstaged(&f);
    let file = model.find("fresh.txt").expect("untracked file");
    stage::stage_partial(
        &f.repo,
        file,
        &[HunkSelection {
            hunk_index: 0,
            lines: Some(vec![0]), // just "alpha"
        }],
    )
    .expect("stage first line");

    assert_eq!(index_content(&f.repo, "fresh.txt"), "alpha\n");
    assert_eq!(
        worktree_content(&f, "fresh.txt"),
        "alpha\nbeta\ngamma\n",
        "the file on disk is untouched"
    );
}

#[test]
fn staging_a_deletion_records_the_removal() {
    let f = Fixture::empty_named("deletion");
    f.commit_file("gone.txt", "content\n", "Add");
    std::fs::remove_file(f.path().join("gone.txt")).expect("remove");

    stage::stage_files(&f.repo, &["gone.txt".to_owned()]).expect("stage deletion");

    let index = f.repo.index().expect("index");
    assert!(
        index.get_path(Path::new("gone.txt"), 0).is_none(),
        "a staged deletion removes the index entry"
    );
}

#[test]
fn unstaging_a_whole_file_restores_the_head_version() {
    let f = three_edits();
    stage::stage_files(&f.repo, &["f.txt".to_owned()]).expect("stage");
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n"
    );

    stage::unstage_files(&f.repo, &["f.txt".to_owned()]).expect("unstage");
    assert_eq!(
        index_content(&f.repo, "f.txt"),
        "one\ntwo\nthree\nfour\nfive\n"
    );
    assert_eq!(
        worktree_content(&f, "f.txt"),
        "ONE\ntwo\nTHREE\nfour\nFIVE\n",
        "unstaging must not touch the working tree"
    );
}

#[test]
fn unstaging_a_file_that_head_does_not_have_makes_it_untracked_again() {
    let f = Fixture::empty_named("unstage_new");
    f.commit_file("seed.txt", "seed\n", "Seed");
    f.write("added.txt", "new\n");
    stage::stage_files(&f.repo, &["added.txt".to_owned()]).expect("stage");

    stage::unstage_files(&f.repo, &["added.txt".to_owned()]).expect("unstage");

    let index = f.repo.index().expect("index");
    assert!(index.get_path(Path::new("added.txt"), 0).is_none());
    assert!(
        f.path().join("added.txt").exists(),
        "the file itself must survive"
    );
}

#[test]
fn discarding_a_file_restores_it_from_the_index() {
    let f = three_edits();
    stage::discard_files(&f.repo, &["f.txt".to_owned()]).expect("discard");
    assert_eq!(
        worktree_content(&f, "f.txt"),
        "one\ntwo\nthree\nfour\nfive\n"
    );
}

#[test]
fn stage_all_picks_up_additions_and_deletions_together() {
    let f = Fixture::dirty();
    stage::stage_all(&f.repo).expect("stage all");

    let status = gitup::git::status::status(&f.repo, false).expect("status");
    assert_eq!(
        status.unstaged_count,
        0,
        "nothing should be left unstaged: {:?}",
        status
            .entries
            .iter()
            .filter(|e| e.has_unstaged())
            .map(|e| (&e.path, e.unstaged))
            .collect::<Vec<_>>()
    );
    assert!(status.staged_count > 0);
}
