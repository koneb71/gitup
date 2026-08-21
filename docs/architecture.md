# Architecture

Gitup is a single native Rust binary. There is no webview, no IPC boundary, and
no serialization layer between the interface and the Git data — the UI reads the
same structs the worker threads produce.

```
src/
  main.rs   window entry point; everything of substance is in the library
  app.rs    state transitions and the frame loop
  job/      the worker pool and the job vocabulary — the UI/Git boundary
  git/      everything libgit2, plus the bridge to the git binary
  ui/       the presentation layer, one module per view
  state.rs  sessions, snapshots, and the selection model
  watch.rs  the filesystem watcher
```

Three decisions account for most of the shape.

## The UI thread never calls libgit2

egui is immediate-mode: it rebuilds and redraws the entire interface every
frame. A revwalk over a hundred thousand commits, or a status scan on a large
worktree, would freeze the window for as long as it took.

So every Git operation goes through the job system in `src/job/`, which owns a
pool of worker threads, each holding its own `git2::Repository`. Results come
back over a channel as immutable snapshots that the UI swaps in wholesale.

```
UI thread (eframe, 60fps)          Workers (own Repository handles)
        │                                          │
        ├── dispatch(Job) ────────────────────────>│  libgit2 / git CLI
        │                                          │
        │<──────── Outcome / Progress ─────────────┤  crossbeam channel
        │                                          │
   apply to the session, ctx.request_repaint()
```

Reads run on a pool; writes are serialized onto a single thread, so two index
mutations can never interleave.

### Topics and supersession

Every job carries a **topic**, and dispatching on a topic supersedes anything
still in flight for it. That is what stops the diff pane flickering through
intermediate commits when you scroll history quickly: only the newest request
for the diff slot survives.

A topic names the repository it concerns. This is not a detail. Supersession
works by replacing the outstanding job on a topic, so a topic shared between two
repositories means loading one repository's history cancels loading another's —
and with several tabs open, every tab but the last is left permanently showing a
skeleton.

Mutations are the exception: they are never superseded, because a half-applied
stage that got cancelled because the user clicked elsewhere is corruption. They
still name their repository, so their progress and failures reach the right tab.

The corollary is that **the caller must not keep its own record of what is in
flight**. A flag set before dispatch survives a job that was superseded or
failed, and nothing ever arrives to clear it — so the feature it guards stops
working permanently. Ask `JobSystem::is_pending` instead, which is right by
construction: a cancelled request is simply no longer pending.

## Network operations shell out to `git`

libgit2 does not implement `credential.helper`, does not read `~/.ssh/config`,
and knows nothing about the macOS keychain, Git Credential Manager, or
`libsecret`. A client built on it either reimplements all of that badly or asks
the user to paste a token — which is exactly the complaint GitAhead accumulated
for years.

Running the real binary inherits every one of those mechanisms, already
configured and already working in the user's terminal. `src/git/cli.rs` handles
it: `GIT_TERMINAL_PROMPT=0` so a missing credential fails with a message instead
of blocking forever on a terminal that does not exist, `LC_ALL=C` so progress
parsing is not locale-dependent, and `CREATE_NO_WINDOW` on Windows so a fetch
does not flash a console over the interface.

There is also no search index. `git log` already searches messages, content,
authors, and paths using data git maintains anyway, so a search is a process
spawn rather than a second copy of the repository that can disagree with the
first — which is the other thing GitAhead's users learned to distrust.

## Partial staging synthesizes patches

Git has no "stage these lines" primitive. Staging a selection means generating a
patch containing only the selected changes and applying it to the index.

The rules are not symmetric. Unselected deletions become context; unselected
additions vanish; hunk headers have to be recomputed for the line counts that
result. Unstaging mirrors all of it, anchored at the *new* line numbers rather
than the old ones, and the `diff --git` header paths swap. `src/git/stage.rs`
documents each rule where it is applied, and it carries the densest test
coverage in the project because every one of those rules was got wrong at least
once.

## Sessions and tabs

A **session** is one open repository: its own selection, search, half-written
commit message, watcher, and loaded snapshots.

The session for the visible tab is held inline on the app rather than indexed
out of a list, because almost every method touches both it and the rest of the
app, and borrowing one out of a `Vec` while mutating the other does not work.
The cost is that "which tab is where" has to be computed rather than read off —
so that arithmetic lives in `state::tab_index` as pure functions with tests,
after an early version quietly reordered the user's tabs on every switch.

Jobs carry a session id from the moment a repository is asked for, before it has
a path to be identified by. Without that, opening a second repository while the
first was still loading delivered the first one's result into the wrong tab —
and a failed open, which never gets a repository key at all, blanked whichever
tab happened to be visible while leaving the tab that actually failed skeletal
forever.

Every outcome is routed to the session that owns it, never to `self.current`.

## Rendering the hard parts

**The commit graph** is computed in a worker: a topological revwalk with an
active-lane algorithm emitting `{ lane, color, edges }` per row. Two lanes are
allowed to expect the same parent, which is what makes branches converge at a
merge instead of shifting the trunk sideways. The UI uses
`ScrollArea::show_rows` so only visible rows are laid out, and paints the lanes,
curves, and dots with `egui::Painter` directly. A hundred-thousand-commit
repository scrolls at full frame rate because roughly forty rows exist per
frame.

**The diff view** is virtualized the same way, one row per line, with syntax
highlighting computed off-thread by `syntect` into `LayoutJob` values and cached
per file and theme. Word-level intra-line diff comes from `similar`.

Both need `item_spacing.y = 0.0`: egui's default four pixels between items
compounds across a virtualized list until the rows the scroll area thinks it is
showing are not the rows on screen.

**Fonts** are embedded rather than taken from the system, so the app looks the
same everywhere. Icons get a dedicated `FontFamily`, because Inter v4 and
Phosphor both claim codepoints in the private use area and whichever loads
second wins.

## The centre split

The commit graph and the detail pane below it divide the centre, and where that
line falls is stored as a **share of the usable centre** rather than a pixel
height (`src/ui/layout.rs`).

Both details are load-bearing. A pixel height hands every extra pixel of a
large screen to the graph, which is only navigation — the diff is the thing
being read, so it should grow with the window. And the share is of the *usable*
centre, past the fixed furniture, because the detail pane pays for two header
bands and the commit box out of its own share: a plain 66% of the centre leaves
the diff 301px at 1280x820 but only 129px at the 880x560 minimum, since that
furniture costs the same at either size.

egui keeps panel sizes in memory only, and eframe's `persistence` feature is
off, so nothing survives a restart on its own. Dragged sizes are written to
Gitup's settings when the drag ends — not while it is happening, which would
rewrite the file every frame.

## Refreshing

`src/watch.rs` watches the worktree and the `.git` directory with a debouncer,
and classifies each path: lock files and `objects/` and `logs/` writes are
ignored, since a fetch writes thousands of objects and none of them change what
is displayed. Every open tab is watched, not just the visible one — a repository
that went into conflict while you were looking elsewhere should say so.

## Further reading

- [building.md](building.md) — toolchains, system libraries, packaging
- [testing.md](testing.md) — how the suite is organized and why snapshots are
  macOS-only
- [platform-notes.md](platform-notes.md) — where behaviour differs by platform
- [shortcuts.md](shortcuts.md) — key bindings and how matching works
