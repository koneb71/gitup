//! Who commits are authored as.
//!
//! Git resolves `user.name` and `user.email` through a chain — system, then
//! global, then the repository's own config — and the last one to set a key
//! wins. That is exactly the sort of thing a settings screen gets wrong: it
//! shows one value, the user changes it, and commits keep coming out under a
//! different name because something further down the chain was overriding it.
//!
//! So this reads all three separately and reports them separately: what the
//! global config says, what this repository overrides it with, and what git
//! will actually use. The interface can then show the effective identity as a
//! fact rather than implying that the field being edited is the one in force.
//!
//! The read and write entry points take [`git2::Config`] values rather than
//! finding them, so the rules can be tested against temporary files. A test
//! that resolved the real global config would be editing the developer's own
//! `~/.gitconfig`.

use crate::error::{Error, Result};
use git2::{Config, ConfigLevel, Repository};

/// A name and email, either of which may be unset.
///
/// Empty and unset are the same thing here. Git will happily store
/// `user.email = ""` and then refuse to commit with it, which is a distinction
/// with no use to anyone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// Whether git could author a commit with this.
    pub fn is_complete(&self) -> bool {
        !self.name.trim().is_empty() && !self.email.trim().is_empty()
    }

    /// Whether nothing at all is set.
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty() && self.email.trim().is_empty()
    }

    /// `Ada Lovelace <ada@example.com>`, as git would write it.
    pub fn display(&self) -> String {
        match (self.name.trim(), self.email.trim()) {
            ("", "") => String::new(),
            (name, "") => name.to_owned(),
            ("", email) => format!("<{email}>"),
            (name, email) => format!("{name} <{email}>"),
        }
    }
}

/// Which config file a change should be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `~/.gitconfig` — the default for every repository.
    Global,
    /// This repository's `.git/config`, overriding the global one.
    Repository,
}

/// The identity at each level, and the one that actually applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identities {
    pub global: Identity,
    /// Only what this repository sets itself. Empty when it inherits.
    pub repository: Identity,
    /// What git will use, wherever it came from — including the system config
    /// and `GIT_AUTHOR_*`, which neither field above can show.
    pub effective: Identity,
}

impl Identities {
    /// Whether committing would fail for want of an identity.
    pub fn can_commit(&self) -> bool {
        self.effective.is_complete()
    }

    /// Whether this repository overrides the global identity.
    pub fn is_overridden(&self) -> bool {
        !self.repository.is_empty()
    }

    /// The identity shown for `scope`, which is what an editor should bind to.
    pub fn at(&self, scope: Scope) -> &Identity {
        match scope {
            Scope::Global => &self.global,
            Scope::Repository => &self.repository,
        }
    }
}

/// Read every level for `repo`.
pub fn read(repo: &Repository) -> Result<Identities> {
    let mut config = repo.config()?;
    // A single-level view fails when that file does not exist yet, which is
    // normal rather than an error: it means nothing is set there.
    let local = config.open_level(ConfigLevel::Local).ok();
    let global = open_global_for_read();

    Ok(Identities {
        global: global.map(|mut c| read_level(&mut c)).unwrap_or_default(),
        repository: local.map(|mut c| read_level(&mut c)).unwrap_or_default(),
        effective: read_level(&mut config),
    })
}

/// Read the global identity without a repository, for when none is open.
pub fn read_global() -> Result<Identities> {
    let identity = open_global_for_read()
        .map(|mut c| read_level(&mut c))
        .unwrap_or_default();
    Ok(Identities {
        global: identity.clone(),
        repository: Identity::default(),
        // With no repository there is no local config to override it, so the
        // global identity is the whole answer.
        effective: identity,
    })
}

/// Write `identity` to the config file for `scope`.
///
/// A field left blank removes the key rather than storing an empty string, so
/// clearing a repository override restores the inherited value instead of
/// shadowing it with nothing.
pub fn write(repo: Option<&Repository>, scope: Scope, identity: &Identity) -> Result<()> {
    let mut config = match (scope, repo) {
        (Scope::Global, _) => open_global_for_write()?,
        (Scope::Repository, Some(repo)) => repo.config()?.open_level(ConfigLevel::Local)?,
        (Scope::Repository, None) => {
            return Err(Error::refused(
                "No repository is open, so there is nothing to set an identity for.",
            ))
        }
    };
    write_level(&mut config, identity)
}

/// Read `user.name` and `user.email` out of one config.
///
/// Separate from [`read`] so the rules can be exercised against a temporary
/// file rather than whatever the machine running the tests happens to have.
pub fn read_level(config: &mut Config) -> Identity {
    // A snapshot is what makes repeated reads consistent; without one libgit2
    // may re-read the file between the two lookups.
    let Ok(snapshot) = config.snapshot() else {
        return Identity::default();
    };
    Identity {
        name: snapshot.get_string("user.name").unwrap_or_default(),
        email: snapshot.get_string("user.email").unwrap_or_default(),
    }
}

/// Apply `identity` to one config, removing keys it leaves blank.
pub fn write_level(config: &mut Config, identity: &Identity) -> Result<()> {
    set_or_clear(config, "user.name", identity.name.trim())?;
    set_or_clear(config, "user.email", identity.email.trim())?;
    Ok(())
}

fn set_or_clear(config: &mut Config, key: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        // Removing a key that was never there is the state the caller asked
        // for, not a failure.
        match config.remove(key) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    } else {
        config.set_str(key, value)?;
        Ok(())
    }
}

/// Where the global config lives, honouring `GIT_CONFIG_GLOBAL`.
///
/// git reads that variable and libgit2 does not, so without this Gitup would
/// edit `~/.gitconfig` for someone whose git is pointed elsewhere. It is also
/// what makes the global level safe to test: a test that resolved the real
/// path would be rewriting the identity of whoever ran it.
///
/// `None` means there is no global config yet, which is not an error — it is
/// what a machine looks like before git has ever been configured.
fn global_path() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("GIT_CONFIG_GLOBAL") {
        // An empty value means "no global config" — git's own reading of it,
        // rather than a file whose name is the empty string.
        return (!path.is_empty()).then(|| path.into());
    }
    Config::find_global().ok()
}

/// The global config, or `None` when the user has never had one.
fn open_global_for_read() -> Option<Config> {
    global_path().and_then(|p| Config::open(&p).ok())
}

/// The global config, creating it if this is the user's first identity.
///
/// `find_global` only reports a file that already exists, so a machine where
/// git has never been configured has nothing to open — which is exactly the
/// case where someone is most likely to be filling this in.
fn open_global_for_write() -> Result<Config> {
    if let Some(path) = global_path() {
        return Ok(Config::open(&path)?);
    }

    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or_else(|| Error::refused("Could not find your home directory."))?;
    Ok(Config::open(&home.join(".gitconfig"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config file of its own, in a directory that is removed with it.
    fn scratch() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config");
        let config = Config::open(&path).expect("open config");
        (dir, config)
    }

    #[test]
    fn an_unset_level_reads_as_empty() {
        let (_dir, mut config) = scratch();
        let identity = read_level(&mut config);
        assert!(identity.is_empty());
        assert!(!identity.is_complete());
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        let (_dir, mut config) = scratch();
        let ada = Identity {
            name: "Ada Lovelace".to_owned(),
            email: "ada@example.com".to_owned(),
        };
        write_level(&mut config, &ada).expect("write");
        assert_eq!(read_level(&mut config), ada);
    }

    #[test]
    fn a_blank_field_removes_the_key_rather_than_emptying_it() {
        // The difference matters: `user.email = ""` in a repository shadows the
        // global address with nothing, and commits then fail there and only
        // there. Clearing the box has to mean "inherit again".
        let (_dir, mut config) = scratch();
        write_level(
            &mut config,
            &Identity {
                name: "Ada Lovelace".to_owned(),
                email: "ada@example.com".to_owned(),
            },
        )
        .expect("write");

        write_level(&mut config, &Identity::default()).expect("clear");

        assert!(config.snapshot().unwrap().get_string("user.name").is_err());
        assert!(config.snapshot().unwrap().get_string("user.email").is_err());
    }

    #[test]
    fn clearing_a_level_that_was_never_set_is_not_an_error() {
        let (_dir, mut config) = scratch();
        write_level(&mut config, &Identity::default()).expect("clearing nothing");
    }

    #[test]
    fn whitespace_is_not_an_identity() {
        let (_dir, mut config) = scratch();
        write_level(
            &mut config,
            &Identity {
                name: "   ".to_owned(),
                email: "\t".to_owned(),
            },
        )
        .expect("write");
        assert!(read_level(&mut config).is_empty());
    }

    #[test]
    fn completeness_needs_both_halves() {
        let name_only = Identity {
            name: "Ada".to_owned(),
            email: String::new(),
        };
        assert!(!name_only.is_complete());
        assert!(!name_only.is_empty());

        let both = Identity {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
        };
        assert!(both.is_complete());
    }

    #[test]
    fn the_display_form_is_what_git_would_write() {
        assert_eq!(
            Identity {
                name: "Ada Lovelace".to_owned(),
                email: "ada@example.com".to_owned(),
            }
            .display(),
            "Ada Lovelace <ada@example.com>"
        );
        assert_eq!(Identity::default().display(), "");
    }

    #[test]
    fn an_override_is_reported_only_when_the_repository_sets_something() {
        let inherited = Identities {
            global: Identity {
                name: "Ada".to_owned(),
                email: "ada@example.com".to_owned(),
            },
            repository: Identity::default(),
            effective: Identity {
                name: "Ada".to_owned(),
                email: "ada@example.com".to_owned(),
            },
        };
        assert!(!inherited.is_overridden());
        assert!(inherited.can_commit());

        let overridden = Identities {
            repository: Identity {
                name: "Ada at Work".to_owned(),
                email: String::new(),
            },
            ..inherited.clone()
        };
        // A half-set override still counts: the name is coming from the
        // repository even though the address is not.
        assert!(overridden.is_overridden());
    }

    #[test]
    fn committing_is_blocked_by_the_effective_identity_alone() {
        // A global identity that a repository has shadowed with a blank is the
        // case worth getting right — the global fields look filled in, and
        // commits still fail.
        let shadowed = Identities {
            global: Identity {
                name: "Ada".to_owned(),
                email: "ada@example.com".to_owned(),
            },
            repository: Identity {
                name: "Ada".to_owned(),
                email: String::new(),
            },
            effective: Identity {
                name: "Ada".to_owned(),
                email: String::new(),
            },
        };
        assert!(!shadowed.can_commit());
    }

    #[test]
    fn a_repository_reads_its_own_level_apart_from_the_global_one() {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = Repository::init(dir.path()).expect("init");
        let mut local = repo
            .config()
            .expect("config")
            .open_level(ConfigLevel::Local)
            .expect("local level");
        write_level(
            &mut local,
            &Identity {
                name: "Repo Only".to_owned(),
                email: "repo@example.com".to_owned(),
            },
        )
        .expect("write");

        let found = read(&repo).expect("read");
        assert_eq!(found.repository.name, "Repo Only");
        // Whatever the machine's global config says, the repository's own
        // value is the one in force.
        assert_eq!(found.effective.name, "Repo Only");
        assert!(found.is_overridden());
    }
}
