# Keyboard shortcuts

Gitup shows shortcuts in whichever way the platform writes them: `⇧⌘P` on
macOS, `Ctrl+Shift+P` on Windows and Linux. The binding is the same one — egui's
`COMMAND` modifier is Cmd on a Mac and Ctrl everywhere else — so this page lists
both spellings side by side and the app only ever shows you yours.

> The table below is checked against the code by a test
> (`documented_shortcuts_match_the_default_keymap` in `tests/docs.rs`). If you
> change a default binding and forget this page, the test fails.

## Remappable

Settings → Keyboard rebinds any of these, and warns about duplicates.

| Action | macOS | Windows / Linux |
|---|---|---|
| Command palette | `⌘K` | `Ctrl+K` |
| Open repository | `⌘O` | `Ctrl+O` |
| Search history | `⌘F` | `Ctrl+F` |
| Refresh | `⌘R` | `Ctrl+R` |
| Settings | `⌘,` | `Ctrl+,` |
| Commit | `⌘↵` | `Ctrl+Enter` |
| Draft a commit message | `⇧⌘G` | `Ctrl+Shift+G` |
| Stage all changes | `⇧⌘A` | `Ctrl+Shift+A` |
| Stash changes | `⇧⌘S` | `Ctrl+Shift+S` |
| New branch | `⇧⌘B` | `Ctrl+Shift+B` |
| Fetch all remotes | `⇧⌘F` | `Ctrl+Shift+F` |
| Pull | `⇧⌘L` | `Ctrl+Shift+L` |
| Push | `⇧⌘P` | `Ctrl+Shift+P` |
| Toggle light/dark theme | `⇧⌘T` | `Ctrl+Shift+T` |

## Fixed

These are not remappable. Tab management follows what every tabbed application
already does, and the rest are contextual — what Escape does depends on what is
open, and the arrow keys move whichever list has focus — which a binding table
cannot express.

| Action | macOS | Windows / Linux |
|---|---|---|
| Show tab 1–8 | `⌘1`…`⌘8` | `Ctrl+1`…`Ctrl+8` |
| Show the last tab | `⌘9` | `Ctrl+9` |
| Open a repository in a new tab | `⌘T` | `Ctrl+T` |
| Close this tab | `⌘W` | `Ctrl+W` |
| Previous / next tab | `⇧⌘[` / `⇧⌘]` | `Ctrl+Shift+[` / `Ctrl+Shift+]` |
| Move through the list | `↑` / `↓` | `Up` / `Down` |
| Move between history and the changed files | `→` / `←` | `Right` / `Left` |
| Stage or unstage the selected file | `Space` | `Space` |
| Dismiss, or leave the current view | `⎋` | `Esc` |

`⌘9` goes to the last tab rather than the ninth, matching browsers rather than
counting.

## Working through changes by keyboard

The arrow keys move through whichever list you are working in, and the list
that has them shows a bright marker while the other dims — the same convention
desktop lists have always used, and the only cue that says where a keypress
will land.

With the working tree selected, `→` steps into the changed files and `←` comes
back to history. `Space` stages the selected file, or unstages it when you are
looking at the staged half, and moves on to the next one — so a long list of
changes goes `Space Space Space` rather than one round trip to the mouse per
file.

## Where bindings are stored

In the settings file, under `[keymap]`, one line per action:

```toml
[keymap]
command_palette = "cmd+k"
push = "cmd+shift+p"
```

The stored name is `cmd` on every platform, so a settings file copied from a Mac
to a PC keeps working. `ctrl` and `control` are accepted when hand-editing, as
are `option` and `opt` for `alt`. The file lives at:

| Platform | Path |
|---|---|
| Linux | `~/.config/gitup/settings.toml` |
| macOS | `~/Library/Application Support/dev.gitup.Gitup/settings.toml` |
| Windows | `%APPDATA%\gitup\Gitup\config\settings.toml` |

## A note on how matching works

egui matches modifiers *logically*: a binding for `⌘F` also fires on `⇧⌘F`,
because the pattern only requires the modifiers it names to be present. That
makes the order bindings are tested in load-bearing, and getting it wrong is
silent — the more specific chord simply never runs.

`src/ui/keymap.rs` sorts bindings by how many modifiers they require and tests
the most specific first, so `⌘F` and `⇧⌘F` coexist without anyone having to
think about it. That is also why the settings sheet flags only exact duplicates
as conflicts.
