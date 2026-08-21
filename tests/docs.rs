//! Checks that the documentation still describes the code.
//!
//! Reference tables rot the moment someone changes a default and does not think
//! to grep the docs. These tests make that a build failure instead of something
//! a user discovers by pressing the wrong key.

use gitup::ui::keymap::{Action, ChordStyle, Keymap};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Parse the rows of every `| a | b | c |` table in a markdown document.
///
/// Deliberately not a real markdown parser: the tables here are hand-written
/// and three columns wide, and anything more would be a dependency for one test.
fn table_rows(markdown: &str) -> Vec<Vec<String>> {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|') && line.ends_with('|'))
        // Separator rows: |---|---|---|
        .filter(|line| {
            !line
                .trim_matches(['|', '-', ':', ' '].as_slice())
                .is_empty()
        })
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_owned())
                .collect()
        })
        .collect()
}

#[test]
fn documented_shortcuts_match_the_default_keymap() {
    let doc = read("docs/shortcuts.md");
    let rows = table_rows(&doc);

    // Both spellings are listed, and both have to be right: the whole point of
    // the page is that a Windows reader sees Ctrl and a Mac reader sees ⌘.
    let documented: BTreeMap<String, (String, String)> = rows
        .iter()
        .filter(|row| row.len() == 3)
        .map(|row| (row[0].clone(), (row[1].clone(), row[2].clone())))
        .collect();

    let keymap = Keymap::default();
    for action in Action::all() {
        let Some(chord) = keymap.chord(action) else {
            continue;
        };
        let label = action.label();
        let Some((mac, pc)) = documented.get(label) else {
            panic!(
                "docs/shortcuts.md does not list {label:?}. Add a row: \
                 | {label} | `{}` | `{}` |",
                chord.display_in(ChordStyle::Symbols),
                chord.display_in(ChordStyle::Words),
            );
        };
        assert_eq!(
            (mac.as_str(), pc.as_str()),
            (
                chord.display_in(ChordStyle::Symbols).as_str(),
                chord.display_in(ChordStyle::Words).as_str()
            ),
            "docs/shortcuts.md is out of date for {label:?}"
        );
    }
}

#[test]
fn documented_actions_all_exist() {
    // The other direction: a row for an action that was removed or renamed
    // tells the reader about a shortcut that does nothing.
    let doc = read("docs/shortcuts.md");
    let keymap = Keymap::default();
    let labels: Vec<&str> = Action::all().into_iter().map(Action::label).collect();

    // Only the first table is generated from `Action`; the second lists fixed
    // bindings that have no `Action` at all. They are separated by the heading.
    let remappable = doc
        .split("## Fixed")
        .next()
        .expect("shortcuts.md lost its Fixed section");

    for row in table_rows(remappable) {
        if row.len() != 3 || row[0] == "Action" {
            continue;
        }
        let label = &row[0];
        assert!(
            labels.contains(&label.as_str()),
            "docs/shortcuts.md lists {label:?}, which is not an action"
        );
        assert!(
            Action::all()
                .into_iter()
                .any(|a| a.label() == label && keymap.chord(a).is_some()),
            "docs/shortcuts.md lists {label:?}, which has no default binding"
        );
    }
}

#[test]
fn the_documented_build_dependencies_match_the_container() {
    // docs/building.md tells Debian and Ubuntu users what to install, and the
    // Dockerfile is what CI and the container build actually use. If they
    // disagree, the instructions are wrong for everyone who is not on CI.
    let docs = read("docs/building.md");
    let dockerfile = read("scripts/docker/Dockerfile.linux");

    let packages: Vec<&str> = dockerfile
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("RUN apt-get"))
        .take_while(|line| !line.starts_with("&& rm -rf"))
        .filter_map(|line| line.trim_end_matches('\\').split_whitespace().last())
        .filter(|word| word.starts_with("lib") || word.contains('-') || *word == "cmake")
        .filter(|word| !word.starts_with("--") && *word != "apt-get")
        .collect();

    assert!(
        !packages.is_empty(),
        "found no packages in the Dockerfile; the parser needs updating"
    );
    for package in packages {
        // git is in the image so the tests can run; it is documented separately
        // as a runtime requirement rather than a build one.
        if package == "git" {
            continue;
        }
        assert!(
            docs.contains(package),
            "{package} is installed by the Dockerfile but not mentioned in \
             docs/building.md"
        );
    }
}

#[test]
fn every_doc_link_to_a_repository_file_resolves() {
    // Relative links between docs are easy to break by moving a file and easy
    // to miss, because nothing renders them until someone clicks.
    let root = repo_root();
    let mut checked = 0;

    // The root documents link into docs/ and at each other just as much, and
    // are the ones a newcomer reads first.
    let entries = std::fs::read_dir(root.join("docs"))
        .expect("docs/ is missing")
        .chain(std::fs::read_dir(&root).expect("repository root"));

    for entry in entries {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable doc");
        for target in markdown_link_targets(&text) {
            // External links and in-page anchors are not ours to verify.
            if target.starts_with("http") || target.starts_with('#') {
                continue;
            }
            let target = target.split('#').next().unwrap_or(&target).to_owned();
            let resolved = path.parent().expect("a parent directory").join(&target);
            assert!(
                resolved.exists(),
                "{} links to {target}, which does not exist",
                path.display()
            );
            checked += 1;
        }
    }

    assert!(checked > 0, "no relative links found; the parser is broken");
}

/// The `target` out of every `[text](target)` in a markdown document.
fn markdown_link_targets(markdown: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let bytes: Vec<char> = markdown.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ']' && i + 1 < bytes.len() && bytes[i + 1] == '(' {
            let start = i + 2;
            if let Some(offset) = bytes[start..].iter().position(|c| *c == ')') {
                targets.push(bytes[start..start + offset].iter().collect());
                i = start + offset;
            }
        }
        i += 1;
    }
    targets
}
