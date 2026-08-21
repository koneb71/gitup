# Drafting commit messages

The **Draft** button beside the commit box writes a first message from what you
have staged. `⇧⌘G` on macOS, `Ctrl+Shift+G` elsewhere, or "Draft a commit
message" in the command palette.

## What it is, and what it is not

It is a set of rules, not a language model. Nothing leaves your machine, there
is no key to configure, it works offline, and it answers instantly.

The tradeoff is real and worth stating plainly: **it describes what changed, not
why.** No amount of reading a diff reveals the reason for it, and a tool that
guessed at intent would produce messages that are confidently wrong — the worst
kind, because they read as deliberate. So the draft is a starting point that
saves you typing the mechanical half. The sentence explaining *why* is still
yours to write, and the box is already open and editable.

The other advantage of rules over a model is predictability. A draft that is
wrong in the same way every time is easy to correct, because after a week you
know what it will say before you press the button.

## It matches your repository's style

Gitup reads the subject lines already in your history and drafts in the same
shape.

If most recent subjects look like `feat(parser): handle empty input`, you get:

```
feat(parser): add lexer.rs
```

If they look like `Add empty-input handling`, you get:

```
Add lexer.rs
```

Merge and revert commits are excluded from the vote — `Merge branch 'main'` is
git's wording, not yours, and a busy branch has enough of them to swing the
result.

## What it writes

**One file** is described by name and by what happened to it:

| Staged | Draft |
|---|---|
| `src/parser.rs` added | `Add parser.rs` |
| `src/parser.rs` deleted | `Remove parser.rs` |
| `src/parser.rs` edited | `Update parser.rs` |
| `tokenizer.rs` → `lexer.rs` | `Move tokenizer.rs to lexer.rs` |

**A file with only additions** names what appeared, when it can read the
declarations off the diff:

```
Add parse_header() and Header to parser.rs
```

This only happens when nothing was removed. Once lines have gone as well, the
change reworked something rather than added to it, and "add" would be a claim
about a change that also took things away.

Declarations are recognized for Rust, Python, JavaScript, TypeScript, Go, and
anything else that starts a definition with `fn`, `def`, `class`, `func`,
`function`, `struct`, `enum`, `trait`, `type`, or `interface`. Indented
declarations are skipped: a method added inside an existing type is rarely the
subject of the commit.

**Two or three files** are named rather than counted:

```
Update README.md and lib.rs
```

**More than three** are counted and placed, with the files listed in the body:

```
Update 7 files in src/ui

- Update src/ui/diff.rs
- Update src/ui/graph.rs
…
```

The body only appears when the subject had to give a number. If the subject
already named the files, listing them again would be the same sentence twice.

## The conventional-commit type

When your history uses conventional commits, the type is inferred from where the
changes are:

| Everything staged is in | Type |
|---|---|
| `docs/`, `*.md`, `LICENSE` | `docs` |
| `tests/`, `*_test.*`, `*.spec.*` | `test` |
| `.github/workflows/`, `.gitlab-ci.yml` | `ci` |
| `Cargo.toml`, `package.json`, `Dockerfile`, … | `build` |
| images, fonts, media | `chore` |

Otherwise it comes down to source code, where **`feat` and `fix` are a guess.**
Both are edits to source; nothing in a diff distinguishes a new capability from
a correction. The rule is:

- a new source file was added → `feat`
- files were only deleted or moved → `refactor`
- otherwise → `fix`

That is a coin weighted by experience, not a judgement. Adding a file is more
often new work; editing in place is more often a correction. When it picks
wrong, the type is the first word in an editable box.

Classification order matters where a file could be two things at once:
`tests/fixtures/README.md` is test data, not documentation, so tests are checked
before extensions.

The **scope** is the deepest directory every staged file shares, minus the
container directories every project has — `src`, `lib`, `app`, `source`,
`internal`, `pkg`. Changes across `src/ui/diff.rs` and `src/ui/graph.rs` get
`(ui)`; changes across `src/parser.rs` and `src/main.rs` get no scope, because
`(src)` would say nothing.

## It will not overwrite what you wrote

The button is available when the box is empty, and when it holds a draft Gitup
itself produced — so you can stage more and draft again, as many times as you
like.

The moment you type, the text becomes yours and the button stops offering. Its
tooltip says why. Nothing Gitup writes can ever replace a sentence you wrote,
which is the one failure that would make the feature not worth having.

## When it is unavailable

- **Nothing staged.** The draft describes the commit you are about to make, so
  there has to be one. Unstaged changes are deliberately ignored — a message
  mentioning a file that will not be in the commit is worse than no message.
- **You have written something.** As above.
