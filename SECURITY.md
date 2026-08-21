# Security policy

## Reporting a vulnerability

Please report security issues privately, through
[GitHub's private advisory form](https://github.com/koneb71/gitup/security/advisories/new),
rather than as a public issue.

Include what you did, what happened, and what you think the impact is. A proof
of concept helps but is not required to report something.

You should get an acknowledgement within a week. Gitup is maintained by
volunteers, so please treat any timeline as best-effort rather than a
commitment.

## Scope

Gitup is a desktop application that reads repositories on your machine and runs
`git` on your behalf. The things most worth reporting are:

- **Anything that executes code from repository contents.** Opening a
  repository, viewing a diff, or rendering a file name should never run
  anything. A crafted repository that achieves execution is the highest-value
  report here.
- **Credential exposure.** Gitup never handles credentials itself — network
  operations shell out to `git` so that your existing helpers, SSH agent, and
  keychain do the work. Anything that causes a credential, token, or key to be
  logged, displayed, or written to disk is in scope.
- **Path traversal or writes outside the repository**, particularly through
  submodule paths, symlinks, or `.gitmodules` entries.
- **Argument injection into the `git` subprocess** — a branch, remote, or ref
  name that becomes a flag.

## Out of scope

- Vulnerabilities in `git`, libgit2, or other dependencies. Please report those
  upstream; if a fix needs a version bump here, an ordinary issue is fine.
- The absence of code signing and notarization. This is known and documented in
  [docs/platform-notes.md](docs/platform-notes.md).
- Anything that requires an attacker who can already run code as your user.

## Supported versions

Gitup is pre-1.0. Fixes go onto `main` and into the next release; older releases
are not patched.
