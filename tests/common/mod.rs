//! Fixture repositories.
//!
//! Every repository is built programmatically in a temp directory so tests
//! describe the history they need instead of depending on a checked-in blob.
//! `TempDir` drops the whole thing at the end of the test.

#![allow(dead_code)]

use git2::{Repository, Signature, Time};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Fixed commit timestamp: 2024-01-01T00:00:00Z.
///
/// Commit hashes are derived from the committer time, so `Signature::now`
/// would produce a different history on every run — and the UI displays short
/// hashes, which would make every image snapshot flaky. Pinning the clock makes
/// the entire fixture byte-for-byte reproducible.
const FIXED_TIME: i64 = 1_704_067_200;

pub struct Fixture {
    /// Owns the temp directory, unless this repository is a sibling created
    /// inside another fixture's — in which case that one keeps it alive.
    pub dir: Option<TempDir>,
    pub root: PathBuf,
    pub repo: Repository,
    /// Incremented per commit so successive commits are ordered but still fixed.
    seq: std::cell::Cell<i64>,
}

impl Fixture {
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn path_buf(&self) -> PathBuf {
        self.root.clone()
    }

    /// A brand-new repository with no commits — the unborn-HEAD case.
    ///
    /// `name` becomes the directory name, and therefore the repository name the
    /// UI displays. It is an argument rather than a random temp name so that
    /// snapshots don't change every run.
    pub fn empty_named(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join(name);
        Self::init_at(Some(dir), root)
    }

    /// Another repository beside this one, sharing its temp directory.
    ///
    /// Sharing matters: a sibling can be referred to by a *relative* path,
    /// which keeps anything recording that path — `.gitmodules`, and therefore
    /// the commit that adds it — identical between runs.
    pub fn sibling(&self, name: &str) -> Self {
        let root = self
            .root
            .parent()
            .expect("fixtures live inside a temp directory")
            .join(name);
        Self::init_at(None, root)
    }

    fn init_at(dir: Option<TempDir>, root: PathBuf) -> Self {
        std::fs::create_dir_all(&root).expect("mkdir");
        let repo = Repository::init(&root).expect("git init");
        // Fixed identity so nothing depends on the machine's git config.
        let mut cfg = repo.config().expect("config");
        cfg.set_str("user.name", "Fixture").unwrap();
        cfg.set_str("user.email", "fixture@example.com").unwrap();
        // A fixed initial branch keeps the branch chip stable regardless of the
        // machine's `init.defaultBranch`.
        repo.set_head("refs/heads/main").expect("set_head");
        Self {
            dir,
            root,
            repo,
            seq: std::cell::Cell::new(0),
        }
    }

    pub fn empty() -> Self {
        Self::empty_named("fixture")
    }

    fn signature(&self) -> Signature<'static> {
        let n = self.seq.get();
        self.seq.set(n + 1);
        Signature::new(
            "Fixture",
            "fixture@example.com",
            &Time::new(FIXED_TIME + n * 3600, 0),
        )
        .expect("signature")
    }

    /// Write a file relative to the worktree, creating parent directories.
    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub fn remove(&self, rel: &str) {
        let _ = std::fs::remove_file(self.root.join(rel));
    }

    /// Stage a path.
    pub fn stage(&self, rel: &str) {
        let mut index = self.repo.index().expect("index");
        index.add_path(Path::new(rel)).expect("add");
        index.write().expect("index write");
    }

    pub fn stage_all(&self) {
        let mut index = self.repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add_all");
        index.write().expect("index write");
    }

    /// Reload the cached index from disk.
    ///
    /// Needed whenever a fixture mixes libgit2 with the `git` binary: each
    /// holds its own view of the index, and the one in this handle does not
    /// notice writes made through the other.
    pub fn reload_index(&self) {
        if let Ok(mut index) = self.repo.index() {
            let _ = index.read(false);
        }
    }

    /// Commit whatever is currently staged.
    pub fn commit(&self, message: &str) -> git2::Oid {
        let sig = self.signature();
        self.reload_index();
        let mut index = self.repo.index().expect("index");
        let tree_id = index.write_tree().expect("write_tree");
        let tree = self.repo.find_tree(tree_id).expect("find_tree");

        let parents = match self.repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(c) => vec![c],
            None => vec![],
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
    }

    /// Convenience: write, stage, commit.
    pub fn commit_file(&self, rel: &str, contents: &str, message: &str) -> git2::Oid {
        self.write(rel, contents);
        self.stage(rel);
        self.commit(message)
    }

    /// Create a branch pointing at the current HEAD.
    pub fn branch(&self, name: &str) {
        let head = self
            .repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .expect("head commit");
        self.repo.branch(name, &head, true).expect("branch");
    }

    /// Point HEAD at a branch and check out its tree.
    pub fn checkout(&self, name: &str) {
        let refname = format!("refs/heads/{name}");
        let obj = self.repo.revparse_single(&refname).expect("revparse");
        self.repo
            .checkout_tree(&obj, Some(git2::build::CheckoutBuilder::new().force()))
            .expect("checkout_tree");
        self.repo.set_head(&refname).expect("set_head");
    }

    /// Commit with HEAD and `other` as parents, keeping HEAD's tree.
    ///
    /// The tree is not merged — these fixtures exist to shape the *graph*, and
    /// a real content merge would only add noise to a lane-assignment test.
    pub fn merge(&self, other: &str, message: &str) -> git2::Oid {
        let sig = self.signature();
        let head = self
            .repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .expect("head commit");
        let other_commit = self
            .repo
            .revparse_single(&format!("refs/heads/{other}"))
            .and_then(|o| o.peel_to_commit())
            .expect("other commit");
        let tree = head.tree().expect("tree");
        self.repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                message,
                &tree,
                &[&head, &other_commit],
            )
            .expect("merge commit")
    }

    /// Create a lightweight tag at HEAD.
    pub fn tag(&self, name: &str) {
        let head = self
            .repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .expect("head commit");
        self.repo
            .tag_lightweight(name, head.as_object(), true)
            .expect("tag");
    }

    /// `main` with a `feature` branch merged back into it.
    ///
    /// ```text
    ///   M   merge
    ///   |\
    ///   B F
    ///   |/
    ///   A
    /// ```
    pub fn merged() -> Self {
        let f = Self::empty_named("merged");
        f.commit_file("a.txt", "a\n", "A");
        f.branch("feature");
        f.commit_file("b.txt", "b\n", "B");
        f.checkout("feature");
        f.commit_file("f.txt", "f\n", "F");
        f.checkout("main");
        f.merge("feature", "Merge feature");
        f
    }

    /// A history with enough shape to exercise lane rendering: two side
    /// branches, one merged back and one still open, plus a tag.
    ///
    /// ```text
    ///   *   feature-b tip
    ///   | *   Merge feature-a   <- main, v0.2
    ///   | |\
    ///   | | * Refine parser
    ///   | | * Add parser
    ///   | |/
    ///   |/|
    ///   * | Start feature-b
    ///   |/
    ///   *   Add config          <- v0.1
    ///   *   Initial commit
    /// ```
    pub fn branchy() -> Self {
        let f = Self::empty_named("branchy");
        f.commit_file("README.md", "# Branchy\n", "Initial commit");
        f.commit_file("config.toml", "debug = true\n", "Add config");
        f.tag("v0.1");

        f.branch("feature-b");
        f.branch("feature-a");

        f.checkout("feature-b");
        f.commit_file("b1.txt", "b1\n", "Start feature-b");

        f.checkout("feature-a");
        f.commit_file("parser.rs", "fn parse() {}\n", "Add parser");
        f.commit_file(
            "parser.rs",
            "fn parse() -> bool { true }\n",
            "Refine parser",
        );

        f.checkout("main");
        f.merge("feature-a", "Merge feature-a");
        f.tag("v0.2");

        // Leave feature-b ahead and unmerged, so one lane stays open.
        f.checkout("feature-b");
        f.commit_file("b2.txt", "b2\n", "Continue feature-b");
        f.checkout("main");
        f
    }

    /// A repository whose latest commit is a realistic source-code edit, for
    /// exercising syntax highlighting and multi-hunk rendering.
    pub fn source() -> Self {
        let f = Self::empty_named("source");
        f.commit_file(
            "src/parser.rs",
            "use std::collections::HashMap;\n\n/// Parses configuration entries.\npub struct Parser {\n    entries: HashMap<String, String>,\n    strict: bool,\n}\n\nimpl Parser {\n    pub fn new() -> Self {\n        Self {\n            entries: HashMap::new(),\n            strict: false,\n        }\n    }\n\n    pub fn parse(&mut self, input: &str) -> Result<usize, String> {\n        let mut count = 0;\n        for line in input.lines() {\n            if line.is_empty() {\n                continue;\n            }\n            let (key, value) = line.split_once('=').unwrap();\n            self.entries.insert(key.to_string(), value.to_string());\n            count += 1;\n        }\n        Ok(count)\n    }\n}\n",
            "Add configuration parser",
        );
        f.commit_file(
            "src/parser.rs",
            "use std::collections::HashMap;\n\n/// Parses configuration entries.\npub struct Parser {\n    entries: HashMap<String, String>,\n    strict: bool,\n}\n\nimpl Parser {\n    pub fn new() -> Self {\n        Self {\n            entries: HashMap::new(),\n            strict: true,\n        }\n    }\n\n    pub fn parse(&mut self, input: &str) -> Result<usize, String> {\n        let mut count = 0;\n        for (number, line) in input.lines().enumerate() {\n            if line.is_empty() || line.starts_with('#') {\n                continue;\n            }\n            let (key, value) = line\n                .split_once('=')\n                .ok_or_else(|| format!(\"line {number}: expected `key = value`\"))?;\n            self.entries.insert(key.trim().to_string(), value.trim().to_string());\n            count += 1;\n        }\n        Ok(count)\n    }\n}\n",
            "Report parse errors instead of panicking",
        );
        f
    }

    /// Two branches that changed the same line, so merging them conflicts.
    ///
    /// ```text
    ///   theirs   "b" -> "THEIRS"
    ///   ours     "b" -> "OURS"      <- main
    ///   base     "a\nb\nc"
    /// ```
    pub fn conflicting() -> Self {
        let f = Self::empty_named("conflict");
        f.commit_file("shared.txt", "a\nb\nc\n", "Base");
        f.branch("theirs");

        f.commit_file("shared.txt", "a\nOURS\nc\n", "Ours changes b");

        f.checkout("theirs");
        f.commit_file("shared.txt", "a\nTHEIRS\nc\n", "Theirs changes b");
        f.checkout("main");
        f
    }

    /// A branch that can be merged with no conflict, because the two sides
    /// touched different files.
    pub fn mergeable() -> Self {
        let f = Self::empty_named("mergeable");
        f.commit_file("base.txt", "base\n", "Base");
        f.branch("side");

        f.commit_file("ours.txt", "ours\n", "Ours adds a file");
        f.checkout("side");
        f.commit_file("theirs.txt", "theirs\n", "Theirs adds a file");
        f.checkout("main");
        f
    }

    /// A linear history of `count` commits on `main`, for rebase planning.
    pub fn linear(count: usize) -> Self {
        let f = Self::empty_named("linear");
        for i in 1..=count {
            f.commit_file(
                &format!("f{i}.txt"),
                &format!("content {i}\n"),
                &format!("Commit {i}"),
            );
        }
        f
    }

    /// A synthetic history of `count` commits, built fast enough to use in a
    /// test.
    ///
    /// Commits are written straight to the object database rather than through
    /// the index and the working tree: for ten thousand commits, the filesystem
    /// round-trips dominate everything else and would make the test unusable.
    pub fn synthetic(name: &str, count: usize) -> Self {
        let f = Self::empty_named(name);
        let mut parent: Option<git2::Oid> = None;

        for i in 0..count {
            let sig = f.signature();
            let blob = f
                .repo
                .blob(format!("content {i}\n").as_bytes())
                .expect("blob");
            let mut builder = f.repo.treebuilder(None).expect("treebuilder");
            builder.insert("file.txt", blob, 0o100_644).expect("insert");
            let tree_id = builder.write().expect("write tree");
            let tree = f.repo.find_tree(tree_id).expect("find tree");

            let parents: Vec<git2::Commit<'_>> = parent
                .into_iter()
                .filter_map(|oid| f.repo.find_commit(oid).ok())
                .collect();
            let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

            parent = Some(
                f.repo
                    .commit(
                        Some("HEAD"),
                        &sig,
                        &sig,
                        &format!("Commit {i}"),
                        &tree,
                        &refs,
                    )
                    .expect("commit"),
            );
        }
        f
    }

    /// Write a solid-colour PNG, for exercising the image diff.
    pub fn write_png(&self, rel: &str, width: u32, height: u32, colour: [u8; 3]) {
        let mut buffer = image::RgbImage::new(width, height);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgb(colour);
        }
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        buffer.save(&path).expect("write png");
    }

    /// A repository whose latest commit replaces an image with a different one.
    pub fn image_change() -> Self {
        let f = Self::empty_named("images");
        f.write_png("logo.png", 96, 64, [242, 133, 63]);
        f.stage("logo.png");
        f.commit("Add the logo");

        f.write_png("logo.png", 128, 64, [92, 168, 255]);
        f.stage("logo.png");
        f.commit("Redraw the logo");
        f
    }

    /// Write a Git LFS pointer file, and optionally the object it points at.
    ///
    /// Real `git lfs` is not needed: a pointer is a text file and the object
    /// store is a directory, so a fixture can build both.
    pub fn write_lfs_pointer(&self, rel: &str, content: &[u8], store_object: bool) -> String {
        // git-lfs keys objects by the sha256 of their contents. The digest only
        // has to be stable and unique here, so a simple hash of the bytes is
        // enough — nothing verifies it.
        let digest: String = {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in content {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            // 64 hex characters, like a real sha256.
            format!("{hash:016x}").repeat(4)
        };

        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{digest}\nsize {}\n",
            content.len()
        );
        self.write(rel, &pointer);

        if store_object {
            let object = self
                .repo
                .path()
                .join("lfs")
                .join("objects")
                .join(&digest[0..2])
                .join(&digest[2..4])
                .join(&digest);
            std::fs::create_dir_all(object.parent().expect("parent")).expect("mkdir");
            std::fs::write(&object, content).expect("write lfs object");
        }
        digest
    }

    /// A parent repository with one submodule, cloned and checked out.
    ///
    /// Returns both fixtures: the child has to stay alive, because its temp
    /// directory *is* the submodule's remote.
    pub fn with_submodule() -> (Self, Self) {
        let parent = Self::empty_named("parent");
        parent.commit_file("README.md", "# Parent\n", "Parent first commit");

        let child = parent.sibling("child");
        child.commit_file("lib.txt", "library\n", "Library first commit");

        // A relative URL, so `.gitmodules` — and the commit that adds it — is
        // byte-for-byte the same every run. An absolute temp path would make
        // the parent's tree hash, and every snapshot showing it, change.
        //
        // git refuses a local path as a submodule source unless told it is
        // allowed; that protection is not what these tests are about.
        crate::common::run_git(
            parent.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--",
                "../child",
                "vendor/lib",
            ],
        );
        parent.commit("Add the submodule");
        (parent, child)
    }

    /// A small clean history on `main`.
    pub fn simple() -> Self {
        Self::simple_named("fixture")
    }

    pub fn simple_named(name: &str) -> Self {
        let f = Self::empty_named(name);
        f.commit_file(
            "README.md",
            "# Fixture\n\nA test repository.\n",
            "Add README",
        );
        f.commit_file(
            "src/main.rs",
            "fn main() {\n    println!(\"hello\");\n}\n",
            "Add main",
        );
        f.commit_file(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
            "Add lib",
        );
        f
    }

    /// A history plus a working tree containing every kind of change the
    /// status view needs to render: staged, unstaged, both, untracked, deleted.
    pub fn dirty() -> Self {
        Self::dirty_named("fixture")
    }

    pub fn dirty_named(name: &str) -> Self {
        let f = Self::simple_named(name);

        // Committed first, and deleted from the worktree further down. This
        // has to happen *before* anything else is staged: `commit` writes the
        // whole index, so staging first would sweep those changes into this
        // commit and the fixture would silently have nothing staged.
        f.commit_file("obsolete.txt", "delete me\n", "Add obsolete");

        // Staged modification.
        f.write("README.md", "# Fixture\n\nEdited and staged.\n");
        f.stage("README.md");

        // Staged *and* then further modified — the `MM` case.
        f.write(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b + 0\n}\n",
        );
        f.stage("src/lib.rs");
        f.write(
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b + 1\n}\n",
        );

        // Unstaged modification only.
        f.write(
            "src/main.rs",
            "fn main() {\n    println!(\"hello, world\");\n}\n",
        );

        // Untracked.
        f.write("notes.txt", "scratch\n");
        f.write("docs/design.md", "# Design\n");

        // Deleted from the worktree but still in the index.
        f.remove("obsolete.txt");

        f
    }
}

/// Run a git command in `workdir`, failing the test if it does not succeed.
pub fn run_git(workdir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
