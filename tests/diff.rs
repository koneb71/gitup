//! The diff engine: line numbering, rename detection, and the guards that keep
//! a pathological file from freezing the renderer.

mod common;

use common::Fixture;
use gitup::git::diff::{self, DiffTarget, LineKind, Omitted};
use gitup::git::highlight::HighlightTheme;
use gitup::job::Cancel;

fn commit_diff(f: &Fixture, summary: &str) -> std::sync::Arc<diff::DiffModel> {
    let page = gitup::git::graph::build(&f.repo, 100, &Cancel::default()).expect("graph");
    let oid = page
        .rows
        .iter()
        .find(|r| r.commit.summary == summary)
        .unwrap_or_else(|| panic!("no commit {summary:?}"))
        .commit
        .id;
    diff::build(
        &f.repo,
        DiffTarget::Commit(oid),
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("diff")
}

#[test]
fn a_root_commit_is_entirely_additions() {
    let f = Fixture::simple();
    let model = commit_diff(&f, "Add README");

    assert_eq!(model.files.len(), 1);
    let file = &model.files[0];
    assert_eq!(file.path, "README.md");
    assert_eq!(file.deletions, 0);
    assert!(file.additions > 0);
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .all(|l| l.kind == LineKind::Addition),
        "a first commit has nothing to remove"
    );
}

#[test]
fn line_numbers_are_resolved_on_both_sides() {
    let f = Fixture::empty_named("lines");
    f.commit_file("f.txt", "one\ntwo\nthree\nfour\nfive\n", "Add");
    f.commit_file("f.txt", "one\nTWO\nthree\nfour\nfive\n", "Change line two");

    let model = commit_diff(&f, "Change line two");
    let file = &model.files[0];
    assert_eq!(file.additions, 1);
    assert_eq!(file.deletions, 1);

    let hunk = &file.hunks[0];
    let removed = hunk
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Deletion)
        .expect("a removed line");
    let added = hunk
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Addition)
        .expect("an added line");

    assert_eq!(removed.content, "two");
    assert_eq!(removed.old_lineno, Some(2));
    assert_eq!(removed.new_lineno, None, "a removed line has no new number");

    assert_eq!(added.content, "TWO");
    assert_eq!(added.new_lineno, Some(2));
    assert_eq!(added.old_lineno, None, "an added line has no old number");

    // Context carries both, which is what lets the side-by-side view align.
    let context = hunk
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Context)
        .expect("context");
    assert!(context.old_lineno.is_some() && context.new_lineno.is_some());
}

#[test]
fn renames_are_detected_rather_than_shown_as_add_plus_delete() {
    let f = Fixture::empty_named("rename");
    let body = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
    f.commit_file("old_name.rs", &body, "Add file");

    std::fs::rename(f.path().join("old_name.rs"), f.path().join("new_name.rs")).expect("rename");
    f.stage_all();
    // `add_all` doesn't record the removal of the old path on its own.
    let mut index = f.repo.index().expect("index");
    index
        .remove_path(std::path::Path::new("old_name.rs"))
        .expect("remove");
    index.write().expect("write");
    f.commit("Rename file");

    let model = commit_diff(&f, "Rename file");
    assert_eq!(model.files.len(), 1, "a rename is one entry, not two");
    let file = &model.files[0];
    assert_eq!(file.path, "new_name.rs");
    assert_eq!(file.old_path.as_deref(), Some("old_name.rs"));
    assert_eq!(file.status, gitup::git::Delta::Renamed);
}

#[test]
fn binary_files_are_flagged_not_rendered() {
    let f = Fixture::empty_named("binary");
    let bytes: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    std::fs::write(f.path().join("blob.bin"), &bytes).expect("write");
    f.stage("blob.bin");
    f.commit("Add binary");

    let model = commit_diff(&f, "Add binary");
    let file = &model.files[0];
    assert_eq!(file.omitted, Some(Omitted::Binary));
    assert!(file.hunks.is_empty(), "binary content must not be rendered");
}

#[test]
fn enormous_files_are_summarized_not_rendered() {
    let f = Fixture::empty_named("huge");
    let body: String = (0..40_000).map(|i| format!("line {i}\n")).collect();
    f.commit_file("huge.txt", &body, "Add huge file");

    let model = commit_diff(&f, "Add huge file");
    let file = &model.files[0];
    assert_eq!(
        file.omitted,
        Some(Omitted::TooLarge),
        "a 40k-line diff must not be handed to the renderer"
    );
    assert!(file.hunks.is_empty());
}

#[test]
fn staged_and_unstaged_are_separate_views() {
    let f = Fixture::dirty();

    let staged = diff::build(
        &f.repo,
        DiffTarget::Staged,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("staged");
    let unstaged = diff::build(
        &f.repo,
        DiffTarget::Unstaged,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("unstaged");

    let staged_paths: Vec<&str> = staged.files.iter().map(|f| f.path.as_str()).collect();
    let unstaged_paths: Vec<&str> = unstaged.files.iter().map(|f| f.path.as_str()).collect();

    assert!(staged_paths.contains(&"README.md"), "got {staged_paths:?}");
    assert!(
        !unstaged_paths.contains(&"README.md"),
        "a fully staged edit is not also unstaged: {unstaged_paths:?}"
    );

    // Staged then edited again appears in both, with different content.
    assert!(staged_paths.contains(&"src/lib.rs"));
    assert!(unstaged_paths.contains(&"src/lib.rs"));

    // Untracked files show their content as additions.
    assert!(
        unstaged_paths.contains(&"notes.txt"),
        "got {unstaged_paths:?}"
    );
    let notes = unstaged.find("notes.txt").expect("notes.txt");
    assert!(notes.additions > 0, "untracked content should be visible");
}

#[test]
fn a_merge_diffs_against_its_first_parent_and_says_so() {
    let f = Fixture::merged();
    let model = commit_diff(&f, "Merge feature");
    assert!(
        model.is_merge_first_parent,
        "the view needs to disclose that this is a first-parent diff"
    );
}

#[test]
fn highlighting_is_applied_to_known_file_types() {
    let f = Fixture::empty_named("highlight");
    f.commit_file(
        "src/lib.rs",
        "pub fn main() {\n    // a comment\n    let x = 42;\n}\n",
        "Add code",
    );

    let page = gitup::git::graph::build(&f.repo, 10, &Cancel::default()).expect("graph");
    let oid = page.rows[0].commit.id;
    let model = diff::build(
        &f.repo,
        DiffTarget::Commit(oid),
        HighlightTheme::Dark,
        &Cancel::default(),
    )
    .expect("diff");

    let line = model.files[0].hunks[0]
        .lines
        .iter()
        .find(|l| l.content.contains("let x"))
        .expect("the assignment line");
    assert!(!line.spans.is_empty(), "Rust should be highlighted");
    let covered: u32 = line.spans.iter().map(|s| s.len).sum();
    assert_eq!(covered as usize, line.display_text().len());

    // With highlighting off the same diff carries no colour at all.
    let plain = diff::build(
        &f.repo,
        DiffTarget::Commit(oid),
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("diff");
    assert!(plain.files[0].hunks[0]
        .lines
        .iter()
        .all(|l| l.spans.is_empty()));
}

#[test]
fn tabs_are_expanded_for_display_but_not_in_the_model() {
    let f = Fixture::empty_named("tabs");
    f.commit_file("t.txt", "\tindented\n", "Add tabbed line");

    let page = gitup::git::graph::build(&f.repo, 10, &Cancel::default()).expect("graph");
    let model = diff::build(
        &f.repo,
        DiffTarget::Commit(page.rows[0].commit.id),
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("diff");

    let line = &model.files[0].hunks[0].lines[0];
    assert_eq!(line.content, "\tindented", "staging needs the real bytes");
    assert_eq!(line.display_text(), "    indented");
}

#[test]
fn changed_images_carry_both_versions() {
    let f = Fixture::image_change();
    let model = commit_diff(&f, "Redraw the logo");
    let file = &model.files[0];

    assert_eq!(file.omitted, Some(Omitted::Binary), "a PNG is still binary");
    let preview = file
        .image
        .as_ref()
        .expect("an image change should carry a preview");

    let old = preview.old.as_ref().expect("the previous version");
    let new = preview.new.as_ref().expect("the new version");
    assert_eq!((old.width, old.height), (96, 64));
    assert_eq!((new.width, new.height), (128, 64));
    assert_ne!(old.bytes, new.bytes);
}

#[test]
fn an_added_image_has_no_before_side() {
    let f = Fixture::empty_named("added_image");
    f.commit_file("seed.txt", "seed\n", "Seed");
    f.write_png("new.png", 32, 32, [80, 200, 120]);
    f.stage("new.png");
    f.commit("Add an image");

    let model = commit_diff(&f, "Add an image");
    let preview = model.files[0].image.as_ref().expect("preview");
    assert!(preview.old.is_none(), "there was nothing before it");
    assert!(preview.new.is_some());
}

#[test]
fn a_non_image_binary_gets_no_preview() {
    let f = Fixture::empty_named("blob_not_image");
    let bytes: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    std::fs::write(f.path().join("data.bin"), &bytes).expect("write");
    f.stage("data.bin");
    f.commit("Add data");

    let model = commit_diff(&f, "Add data");
    assert_eq!(model.files[0].omitted, Some(Omitted::Binary));
    assert!(
        model.files[0].image.is_none(),
        "only recognizable image types get a preview"
    );
}

#[test]
fn an_uncommitted_image_change_reads_the_new_side_from_disk() {
    let f = Fixture::image_change();
    // A working-tree diff has no blob for the new side.
    f.write_png("logo.png", 64, 64, [200, 60, 60]);

    let model = diff::build(
        &f.repo,
        DiffTarget::Unstaged,
        HighlightTheme::Off,
        &Cancel::default(),
    )
    .expect("diff");

    let preview = model
        .find("logo.png")
        .and_then(|f| f.image.as_ref())
        .expect("preview");
    assert_eq!(
        preview.new.as_ref().map(|s| (s.width, s.height)),
        Some((64, 64)),
        "the new side comes from the working tree"
    );
    assert_eq!(
        preview.old.as_ref().map(|s| (s.width, s.height)),
        Some((128, 64)),
        "the old side is the committed blob"
    );
}

#[test]
fn an_lfs_pointer_is_described_by_its_object_not_its_text() {
    let f = Fixture::empty_named("lfs");
    f.write_lfs_pointer("big.bin", &vec![7u8; 4096], false);
    f.stage("big.bin");
    f.commit("Track a big file");

    f.write_lfs_pointer("big.bin", &vec![9u8; 16384], false);
    f.stage("big.bin");
    f.commit("Replace it with a bigger one");

    let model = commit_diff(&f, "Replace it with a bigger one");
    let file = &model.files[0];
    let lfs = file.lfs.as_ref().expect("recognized as LFS");

    assert_eq!(lfs.old.as_ref().map(|p| p.size), Some(4096));
    assert_eq!(lfs.new.as_ref().map(|p| p.size), Some(16384));
    assert_eq!(lfs.size_delta(), Some(12288));
    assert!(
        file.hunks.is_empty(),
        "the pointer's own text must not be diffed"
    );
}

#[test]
fn lfs_reports_whether_the_object_is_downloaded() {
    let f = Fixture::empty_named("lfs_present");
    f.write_lfs_pointer("seed.bin", b"seed", false);
    f.stage("seed.bin");
    f.commit("Seed");

    // Second version, with the object actually stored.
    f.write_lfs_pointer("seed.bin", b"the real contents", true);
    f.stage("seed.bin");
    f.commit("Update");

    let model = commit_diff(&f, "Update");
    let lfs = model.files[0].lfs.as_ref().expect("LFS");
    assert!(
        lfs.new_downloaded,
        "the new object was written to the store"
    );
    assert!(!lfs.old_downloaded, "the old one was not");
}

#[test]
fn a_downloaded_lfs_image_is_still_shown() {
    let f = Fixture::empty_named("lfs_image");
    f.commit_file("seed.txt", "seed\n", "Seed");

    // Build a real PNG, store it as an LFS object, and point at it.
    let mut png = Vec::new();
    let mut buffer = image::RgbImage::new(48, 24);
    for pixel in buffer.pixels_mut() {
        *pixel = image::Rgb([10, 200, 120]);
    }
    buffer
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode");

    f.write_lfs_pointer("art.png", &png, true);
    f.stage("art.png");
    f.commit("Add LFS artwork");

    let model = commit_diff(&f, "Add LFS artwork");
    let file = &model.files[0];
    assert!(file.lfs.is_some());
    let preview = file
        .image
        .as_ref()
        .expect("a downloaded LFS image should still be previewable");
    assert_eq!(
        preview.new.as_ref().map(|s| (s.width, s.height)),
        Some((48, 24))
    );
}

#[test]
fn an_undownloaded_lfs_image_has_no_preview() {
    let f = Fixture::empty_named("lfs_missing");
    f.commit_file("seed.txt", "seed\n", "Seed");
    f.write_lfs_pointer("art.png", b"pretend this is a png", false);
    f.stage("art.png");
    f.commit("Add artwork");

    let model = commit_diff(&f, "Add artwork");
    let file = &model.files[0];
    assert!(file.lfs.is_some(), "still recognized as LFS");
    assert!(
        file.image.is_none(),
        "nothing to preview when the object isn't here"
    );
}

#[test]
fn deletions_are_reported() {
    let f = Fixture::empty_named("deletion");
    f.commit_file("gone.txt", "content\n", "Add");
    std::fs::remove_file(f.path().join("gone.txt")).expect("remove");
    let mut index = f.repo.index().expect("index");
    index
        .remove_path(std::path::Path::new("gone.txt"))
        .expect("remove path");
    index.write().expect("write");
    f.commit("Delete it");

    let model = commit_diff(&f, "Delete it");
    let file = &model.files[0];
    assert_eq!(file.status, gitup::git::Delta::Deleted);
    assert!(file.deletions > 0 && file.additions == 0);
}
