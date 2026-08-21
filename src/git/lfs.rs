//! Git LFS pointer files.
//!
//! LFS replaces a large file in the repository with a small text file naming
//! it. Diffing that honestly produces three lines of metadata — technically
//! correct and completely useless, since the thing that changed is a file you
//! cannot see. This module recognizes pointers so the view can say what the
//! object actually is, whether it is downloaded, and — when it is an image that
//! has been fetched — show it.

use std::path::{Path, PathBuf};

/// The magic first line every pointer file starts with.
const VERSION_PREFIX: &str = "version https://git-lfs.github.com/spec/v1";

/// A pointer file is small by construction; anything larger is real content.
const MAX_POINTER_BYTES: usize = 1024;

/// A parsed LFS pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    /// Hash algorithm and digest, e.g. `sha256:4d7a2146…`.
    pub oid: String,
    /// Size of the real object in bytes.
    pub size: u64,
}

impl Pointer {
    /// The digest without its algorithm prefix, which is what the object store
    /// is keyed by.
    pub fn digest(&self) -> &str {
        self.oid
            .split_once(':')
            .map(|(_, d)| d)
            .unwrap_or(&self.oid)
    }

    /// A shortened digest, for display.
    pub fn short(&self) -> String {
        self.digest().chars().take(12).collect()
    }

    /// Where git-lfs stores this object inside the repository.
    ///
    /// The layout is `lfs/objects/<first two>/<next two>/<full digest>`, which
    /// is git-lfs's own sharding scheme.
    pub fn object_path(&self, git_dir: &Path) -> Option<PathBuf> {
        let digest = self.digest();
        if digest.len() < 4 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(
            git_dir
                .join("lfs")
                .join("objects")
                .join(&digest[0..2])
                .join(&digest[2..4])
                .join(digest),
        )
    }

    /// Whether the real object has been downloaded.
    pub fn is_downloaded(&self, git_dir: &Path) -> bool {
        self.object_path(git_dir).is_some_and(|p| p.is_file())
    }
}

/// Parse `bytes` as an LFS pointer, if that is what they are.
///
/// Deliberately strict about the first line and lenient about the rest: the
/// spec allows extra `ext-*` keys and does not promise an order beyond
/// `version` coming first, so unknown keys are skipped rather than rejected.
pub fn parse(bytes: &[u8]) -> Option<Pointer> {
    if bytes.len() > MAX_POINTER_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();

    // The first line identifies the format. Without it this is just a file.
    if !lines.next()?.trim_end().starts_with(VERSION_PREFIX) {
        return None;
    }

    let (mut oid, mut size) = (None, None);
    for line in lines {
        let Some((key, value)) = line.trim_end().split_once(' ') else {
            continue;
        };
        match key {
            "oid" => oid = Some(value.trim().to_owned()),
            "size" => size = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }

    Some(Pointer {
        oid: oid?,
        size: size?,
    })
}

/// Both versions of a file stored in LFS.
#[derive(Debug, Clone)]
pub struct LfsChange {
    pub old: Option<Pointer>,
    pub new: Option<Pointer>,
    /// Whether the new object is present locally, so the view can say when
    /// something is missing rather than appearing to show nothing.
    pub new_downloaded: bool,
    pub old_downloaded: bool,
}

impl LfsChange {
    /// How the size changed, when both sides are known.
    pub fn size_delta(&self) -> Option<i64> {
        let old = self.old.as_ref()?.size as i64;
        let new = self.new.as_ref()?.size as i64;
        Some(new - old)
    }
}

/// Build a change description from the two sides' raw contents.
pub fn change(old: Option<&[u8]>, new: Option<&[u8]>, git_dir: &Path) -> Option<LfsChange> {
    let old = old.and_then(parse);
    let new = new.and_then(parse);
    if old.is_none() && new.is_none() {
        return None;
    }
    Some(LfsChange {
        old_downloaded: old.as_ref().is_some_and(|p| p.is_downloaded(git_dir)),
        new_downloaded: new.as_ref().is_some_and(|p| p.is_downloaded(git_dir)),
        old,
        new,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINTER: &str = "version https://git-lfs.github.com/spec/v1\n\
                           oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
                           size 12345\n";

    #[test]
    fn a_well_formed_pointer_parses() {
        let pointer = parse(POINTER.as_bytes()).expect("pointer");
        assert_eq!(pointer.size, 12345);
        assert!(pointer.oid.starts_with("sha256:"));
        assert_eq!(pointer.digest().len(), 64);
        assert_eq!(pointer.short(), "4d7a214614ab");
    }

    #[test]
    fn ordinary_files_are_not_pointers() {
        assert!(parse(b"just some text\n").is_none());
        assert!(parse(b"").is_none());
        // Content that merely mentions LFS is not a pointer.
        assert!(parse(b"see https://git-lfs.github.com/spec/v1 for details\n").is_none());
    }

    #[test]
    fn a_pointer_missing_a_required_field_is_rejected() {
        let no_size = "version https://git-lfs.github.com/spec/v1\noid sha256:abc\n";
        assert!(parse(no_size.as_bytes()).is_none());
        let no_oid = "version https://git-lfs.github.com/spec/v1\nsize 10\n";
        assert!(parse(no_oid.as_bytes()).is_none());
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        let with_ext = "version https://git-lfs.github.com/spec/v1\n\
                        ext-0-sha256 comp=zip\n\
                        oid sha256:abcdef0123456789\n\
                        size 7\n";
        let pointer = parse(with_ext.as_bytes()).expect("pointer");
        assert_eq!(pointer.size, 7);
    }

    #[test]
    fn something_too_large_to_be_a_pointer_is_rejected_cheaply() {
        let mut big = POINTER.to_owned();
        big.push_str(&"x".repeat(MAX_POINTER_BYTES));
        assert!(parse(big.as_bytes()).is_none());
    }

    #[test]
    fn binary_content_is_not_a_pointer() {
        assert!(parse(&[0xFF, 0xFE, 0x00, 0x01]).is_none());
    }

    #[test]
    fn the_object_path_matches_git_lfs_sharding() {
        let pointer = parse(POINTER.as_bytes()).expect("pointer");
        let path = pointer.object_path(Path::new("/repo/.git")).expect("path");
        assert!(path.ends_with(
            "lfs/objects/4d/7a/4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
        ), "got {path:?}");
    }

    #[test]
    fn a_non_hex_digest_has_no_object_path() {
        let pointer = Pointer {
            oid: "sha256:not/a/digest".to_owned(),
            size: 1,
        };
        assert!(pointer.object_path(Path::new("/repo/.git")).is_none());
    }

    #[test]
    fn size_delta_needs_both_sides() {
        let make = |size| {
            Some(Pointer {
                oid: "sha256:ab".to_owned(),
                size,
            })
        };
        let both = LfsChange {
            old: make(100),
            new: make(250),
            old_downloaded: false,
            new_downloaded: false,
        };
        assert_eq!(both.size_delta(), Some(150));

        let added = LfsChange {
            old: None,
            new: make(250),
            old_downloaded: false,
            new_downloaded: false,
        };
        assert_eq!(added.size_delta(), None);
    }

    #[test]
    fn a_change_needs_at_least_one_pointer() {
        let git_dir = Path::new("/repo/.git");
        assert!(change(None, None, git_dir).is_none());
        assert!(change(Some(b"plain text"), Some(b"more text"), git_dir).is_none());
        assert!(change(None, Some(POINTER.as_bytes()), git_dir).is_some());
    }
}
