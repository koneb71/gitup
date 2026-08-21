# Identity

**Settings → Identity** (`⌘,` on macOS, `Ctrl+,` elsewhere) sets the name and
email that commits are authored under — the same `user.name` and `user.email`
git reads, written to the same files git reads them from. Nothing is stored in
Gitup's own settings; a change made here is a change made to your git config,
and `git config user.email` in a terminal will report it.

## Two levels

| Applies to | Written to | Meaning |
|---|---|---|
| Every repository | `~/.gitconfig` | your default identity |
| This repository | `.git/config` | overrides the default, here only |

The per-repository level is what you want when one project needs a work address
and everything else uses a personal one. It is also the safer way round: set the
personal address globally, and override it in the repositories that need
something else.

If you have never configured git, the global file may not exist yet. Saving a
global identity creates it.

## The line at the top is the one that counts

Above the fields, the sheet states who commits will actually be authored as:

```
Commits are authored as    Ada Lovelace <ada@example.com>
```

That is deliberate, and it is the reason this screen has three values in it
rather than one. Git resolves `user.name` through a chain — system config, then
global, then the repository's own — and the last one to set a key wins. A
settings screen that showed only the field being edited would let you change
your global address, watch it save, and still produce commits under a different
name, because the repository was overriding it. The effective line is a fact
about what git will do; the fields below are one level each.

If it is missing, Gitup says so, because git will refuse to commit:

> Git will refuse to commit until a name and email are set.

## Clearing a field means inheriting again

Under **This repository**, the global values appear as placeholder text in the
empty fields — that is what a blank means there.

Blanking a field **removes** the key rather than storing an empty string. The
difference is not cosmetic: `user.email = ""` in a repository shadows your
global address with nothing, and commits then fail in that one repository and
nowhere else, which is a confusing thing to debug. Clearing the box restores the
inherited value instead.

## Saving

**Save** is enabled only when the fields differ from what is stored, so the
button doubles as an indication of whether you have unsaved changes. Nothing is
written as you type — a config file rewritten on every keystroke would fight
you, and would leave half-typed addresses on disk.

After saving, Gitup re-reads all three levels rather than assuming the write
took effect as asked. What comes back is what git will use.

## `GIT_CONFIG_GLOBAL`

If that variable is set, Gitup uses it as the global config, exactly as git
does. libgit2 does not honour it on its own, so without this Gitup would edit
`~/.gitconfig` for someone whose git was pointed elsewhere — and the two would
silently disagree about who you are.

An empty value means "no global config", which is also git's reading of it.

## What this does not cover

- **Signing keys** (`user.signingkey`, `commit.gpgsign`). Gitup does not sign
  commits.
- **Per-directory conditional includes** (`includeIf "gitdir:~/work/"`). Git
  supports them and Gitup reads whatever they produce — the effective line will
  show the result — but the sheet edits the global and repository files
  directly rather than authoring conditional includes.
- **System config** (`/etc/gitconfig`). Read as part of the effective identity,
  never written; it usually needs root and is not the user's to change.
