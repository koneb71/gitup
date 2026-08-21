//! The application shell: state transitions, and the frame loop that draws them.

use crate::error::Error;
use crate::git::{repo::HeadKind, RepoKey};
use crate::job::{Job, JobSystem, Message, Outcome};
use crate::job::{Mutation, PartialKind};
use crate::settings::Settings;
use crate::state::{
    CenterView, Selection, Session, Toast, ToastKind, WorkdirView, INITIAL_GRAPH_LIMIT,
};
use crate::ui::keymap::{Action as A, Chord};
use crate::ui::{icons, metrics, radius, space, text, Palette, ThemeMode};
use crate::watch::RepoWatcher;
use crate::APP_NAME;
use crossbeam_channel::{Receiver, Sender};
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Stroke, Vec2};
use std::path::PathBuf;

/// Which session a tab position refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabRef {
    Current,
    Background(usize),
}

/// Which request a folder picker is answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    /// Replace what is in the current tab.
    OpenRepo,
    /// Open in a tab of its own.
    NewTab,
    CloneParent,
}

/// The folder picker's answer, and what it was asked for.
///
/// `None` means the user cancelled — which still has to come back, so the app
/// knows the picker is closed.
type PickedFolder = (PickerKind, Option<PathBuf>);

pub struct GitupApp {
    jobs: JobSystem,
    settings: Settings,
    palette: Palette,

    /// The tab being shown.
    ///
    /// Held inline rather than indexed out of `background` so that borrowing
    /// it never conflicts with borrowing the rest of the app — which matters,
    /// because almost every method touches both.
    current: Session,
    /// Tabs that are open but not on screen.
    ///
    /// These keep receiving job results: a fetch started in one tab must
    /// finish even after the user has moved to another.
    background: Vec<Session>,
    /// Where the current session sits in the tab bar.
    current_slot: usize,
    /// Source of session ids.
    next_session_id: u64,
    /// The title last sent to the window, so it is only set when it changes.
    window_title: String,

    toasts: Vec<Toast>,
    /// The open modal, if any.
    dialog: Option<crate::ui::dialog::Dialog>,
    /// Live progress, per repository.
    ///
    /// Keyed rather than a single slot: two tabs can be fetching at once, and a
    /// global slot means one repository's operation blocks the other's controls
    /// and draws its progress bar in the wrong tab. `None` is the key for work
    /// belonging to no repository yet, which is a clone.
    progress: std::collections::HashMap<Option<RepoKey>, (u64, crate::job::Progress)>,
    /// Branch whose deletion was refused for having unmerged commits, so the
    /// dialog can be re-opened with a force option.
    pending_delete: Option<String>,
    /// True while the search field should hold focus.
    search_focus: bool,
    /// Command palette state.
    palette_open: bool,
    palette_query: String,
    palette_selected: usize,
    palette_just_opened: bool,
    settings_open: bool,
    /// Identity as read for whichever tab was showing when the sheet opened.
    identity: Option<crate::git::identity::Identities>,
    /// The fields being edited, and the config they will be written to.
    identity_draft: crate::git::identity::Identity,
    identity_scope: crate::git::identity::Scope,
    /// True while a pointer button is held, so a panel size is only recorded
    /// when the user is actually dragging a splitter — not on the first frame,
    /// where it would overwrite the proportional default with whatever the
    /// panel happened to report.
    dragging: bool,
    /// Set when a dragged layout size needs writing to the settings file.
    ///
    /// A splitter reports a new size on every frame of a drag, and writing the
    /// file that often would be absurd. Flushing once the pointer comes up
    /// costs one write per drag — and is what makes a resize survive quitting
    /// at all, since eframe's own persistence is off and `App::save` is
    /// therefore never called.
    layout_dirty: bool,
    /// Height the diff body last had, for the test that guards against the
    /// panes above it creeping back over the diff.
    diff_body_height: f32,
    /// The binding being re-recorded in the settings sheet, if any.
    recording_binding: Option<crate::ui::keymap::Action>,
    /// `git --version`, read once and shown in the settings sheet.
    git_version: Option<String>,
    /// Folder picker results. The dialog runs on its own thread so it can never
    /// block a frame.
    picker: (Sender<PickedFolder>, Receiver<PickedFolder>),
    picker_open: bool,
}

impl GitupApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        Self::new_in(&cc.egui_ctx, Settings::load(), initial)
    }

    /// Construct with explicit settings instead of whatever is on disk.
    pub fn new_with(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        initial: Option<PathBuf>,
    ) -> Self {
        Self::new_in(&cc.egui_ctx, settings, initial)
    }

    /// Construct against a bare [`egui::Context`].
    ///
    /// Kept separate from [`Self::new`] so tests can build the app without an
    /// `eframe::CreationContext`, which only exists inside a real window.
    pub fn new_in(ctx: &egui::Context, settings: Settings, initial: Option<PathBuf>) -> Self {
        let palette = Palette::for_mode(settings.theme);

        text::install_fonts(ctx);
        crate::ui::theme::install_styles(ctx);
        crate::ui::theme::set_mode(ctx, settings.theme);
        egui_extras::install_image_loaders(ctx);

        let mut app = Self {
            jobs: JobSystem::new(ctx.clone()),
            settings,
            palette,
            current: Session::default(),
            background: Vec::new(),
            current_slot: 0,
            next_session_id: 0,
            window_title: String::new(),
            toasts: Vec::new(),
            dialog: None,
            progress: std::collections::HashMap::new(),
            pending_delete: None,
            search_focus: false,
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,
            palette_just_opened: false,
            settings_open: false,
            identity: None,
            identity_draft: crate::git::identity::Identity::default(),
            identity_scope: crate::git::identity::Scope::Global,
            dragging: false,
            layout_dirty: false,
            diff_body_height: 0.0,
            recording_binding: None,
            git_version: None,
            picker: crossbeam_channel::unbounded(),
            picker_open: false,
        };

        // Restore what was open. An explicit path still wins, and lands in a
        // tab of its own so it is the one showing.
        let restored = app.settings.restorable_tabs();
        for (index, path) in restored.iter().enumerate() {
            if index == 0 {
                app.open_repo(path.clone());
            } else {
                app.open_in_new_tab(path.clone());
            }
        }
        if let Some(path) = initial {
            if restored.is_empty() {
                app.open_repo(path);
            } else {
                app.open_in_new_tab(path);
            }
        }

        app
    }

    // ------------------------------------------------------------------ tabs

    fn tab_count(&self) -> usize {
        usize::from(self.current.is_open()) + self.background.len()
    }

    /// Tabs in the order they are shown: background tabs keep their order, and
    /// the current one sits wherever it was left.
    ///
    /// The current session is held separately for borrow reasons, so its
    /// position among the others has to be tracked explicitly — otherwise
    /// switching tabs would shuffle them, which makes a tab bar useless.
    fn tab_order(&self) -> Vec<TabRef> {
        let mut refs: Vec<TabRef> = Vec::with_capacity(self.tab_count());
        let mut background = self.background.iter().enumerate();
        for slot in 0..self.tab_count() {
            if slot == self.current_slot {
                refs.push(TabRef::Current);
            } else if let Some((index, _)) = background.next() {
                refs.push(TabRef::Background(index));
            }
        }
        // A mismatch would mean the slot index drifted; putting the current
        // tab last is a harmless recovery rather than a panic.
        if !refs.contains(&TabRef::Current) && self.current.is_open() {
            refs.push(TabRef::Current);
        }
        refs
    }

    fn session_at(&self, tab: TabRef) -> &Session {
        match tab {
            TabRef::Current => &self.current,
            TabRef::Background(index) => &self.background[index],
        }
    }

    /// Show the tab at `position` in the tab bar.
    ///
    /// The outgoing session is put back where it was rather than appended:
    /// switching tabs must not reorder them.
    fn activate(&mut self, position: usize) {
        if position >= self.tab_count() || position == self.current_slot {
            return;
        }
        let Some(index) = crate::state::tab_index::background_of(position, self.current_slot)
        else {
            return;
        };
        if index >= self.background.len() {
            return;
        }

        let incoming = self.background.remove(index);
        let outgoing = std::mem::replace(&mut self.current, incoming);
        let at = crate::state::tab_index::reinsert_at(self.current_slot, position);
        self.background
            .insert(at.min(self.background.len()), outgoing);
        self.current_slot = position;

        self.remember_open_tabs();
    }

    /// Move `delta` tabs along, wrapping.
    fn cycle_tab(&mut self, delta: i32) {
        let count = self.tab_count() as i32;
        if count < 2 {
            return;
        }
        let next = (self.current_slot as i32 + delta).rem_euclid(count);
        self.activate(next as usize);
    }

    fn activate_by_key(&mut self, key: &RepoKey) {
        let order = self.tab_order();
        if let Some(position) = order
            .iter()
            .position(|tab| self.session_at(*tab).repo.key.as_ref() == Some(key))
        {
            self.activate(position);
        }
    }

    /// Open `path` in a new tab, or switch to it if it is already open.
    fn open_in_new_tab(&mut self, path: PathBuf) {
        // Resolving the path here rather than after the job means an
        // already-open repository is recognized before a second one is built.
        if let Ok(key) = crate::git::repo::discover(&path) {
            let order = self.tab_order();
            if order
                .iter()
                .any(|tab| self.session_at(*tab).repo.key.as_ref() == Some(&key))
            {
                self.activate_by_key(&key);
                return;
            }
        }

        // `loading` counts as occupied: a tab whose repository is still being
        // found is still a tab, and overwriting it would lose it.
        if self.current.is_open() || self.current.repo.loading {
            // Park the current tab in its own slot — pushing it to the end
            // would move it whenever it was not already last — and start a
            // fresh one after all of them.
            let parked = std::mem::take(&mut self.current);
            let at = self.current_slot.min(self.background.len());
            self.background.insert(at, parked);
            self.current_slot = self.background.len();
        }
        self.open_repo(path);
    }

    /// Close the tab at `position`.
    fn close_tab(&mut self, position: usize) {
        let order = self.tab_order();
        let Some(&tab) = order.get(position) else {
            return;
        };

        match tab {
            TabRef::Background(index) => {
                self.background.remove(index);
                if self.current_slot > position {
                    self.current_slot -= 1;
                }
            }
            TabRef::Current => {
                // Closing the visible tab: take over whichever one is next, or
                // fall back to the empty state.
                match crate::state::tab_index::successor(position, self.background.len()) {
                    Some(next) => {
                        self.current = self.background.remove(next);
                        self.current_slot = position.min(self.tab_count().saturating_sub(1));
                    }
                    None => {
                        self.current = Session::default();
                        self.current_slot = 0;
                    }
                }
            }
        }

        self.remember_open_tabs();
    }

    /// Remove a session that turned out to be a duplicate.
    fn discard_session(&mut self, id: u64) {
        if self.current.id == id {
            match crate::state::tab_index::successor(self.current_slot, self.background.len()) {
                Some(next) => {
                    self.current = self.background.remove(next);
                    self.current_slot = self.current_slot.min(self.tab_count() - 1);
                }
                None => {
                    self.current = Session::default();
                    self.current_slot = 0;
                }
            }
        } else if let Some(index) = self.background.iter().position(|s| s.id == id) {
            self.background.remove(index);
            if self.current_slot > index {
                self.current_slot -= 1;
            }
        }
    }

    /// Store the open repositories so the next launch restores them.
    fn remember_open_tabs(&mut self) {
        let order = self.tab_order();
        self.settings.open_tabs = order
            .iter()
            .filter_map(|tab| self.session_at(*tab).repo.key.as_ref().map(|k| k.0.clone()))
            .collect();
        self.settings.active_tab = self.current_slot;
        self.settings.save();
    }

    /// Load everything a freshly opened repository needs.
    fn load_repository(&mut self, key: &RepoKey) {
        self.jobs.dispatch(Job::ReadStatus {
            repo: key.clone(),
            include_ignored: self.settings.show_ignored,
        });
        self.jobs.dispatch(Job::LoadGraph {
            repo: key.clone(),
            limit: INITIAL_GRAPH_LIMIT,
        });
        self.jobs.dispatch(Job::LoadRefs(key.clone()));
        self.jobs.dispatch(Job::LoadSubmodules(key.clone()));
    }

    /// Re-read a specific repository, whichever tab holds it.
    /// Re-read everything the tab shows about `key`.
    ///
    /// Dropping the diffs is part of refreshing, not a decision each caller
    /// makes: a diff kept across a refresh is a diff of a state the repository
    /// is no longer in. Every path that used to do this separately — the
    /// refresh command, the watcher — got it right, which is exactly why it
    /// belongs here rather than repeated in each of them.
    fn refresh(&mut self, key: &RepoKey) {
        let limit = self
            .session_for(key)
            .map(|s| s.repo.graph_limit.max(INITIAL_GRAPH_LIMIT))
            .unwrap_or(INITIAL_GRAPH_LIMIT);
        if let Some(session) = self.session_for(key) {
            session.repo.unstaged = None;
            session.repo.staged = None;
        }
        self.jobs.dispatch(Job::ReadHead(key.clone()));
        self.jobs.dispatch(Job::ReadStatus {
            repo: key.clone(),
            include_ignored: self.settings.show_ignored,
        });
        self.jobs.dispatch(Job::LoadGraph {
            repo: key.clone(),
            limit,
        });
        self.jobs.dispatch(Job::LoadRefs(key.clone()));
        self.jobs.dispatch(Job::LoadSubmodules(key.clone()));
    }

    // ---------------------------------------------------------------- actions

    /// A fresh session, with an id nothing else has used.
    fn new_session(&mut self) -> Session {
        self.next_session_id += 1;
        Session {
            id: self.next_session_id,
            ..Session::default()
        }
    }

    /// The session with this id, wherever it is.
    fn session_by_id(&mut self, id: u64) -> Option<&mut Session> {
        if self.current.id == id {
            return Some(&mut self.current);
        }
        self.background.iter_mut().find(|s| s.id == id)
    }

    fn open_repo(&mut self, path: PathBuf) {
        self.current.repo.close();
        self.current.watcher = None;
        self.current.repo.loading = true;
        // The current session is replaced wholesale, and takes a new id: any
        // answer still coming for the old one now belongs to nothing and is
        // correctly ignored.
        let token = {
            let session = self.new_session();
            let id = session.id;
            self.current = session;
            id
        };
        self.current.repo.loading = true;
        self.jobs.dispatch(Job::OpenRepo { path, token });
    }

    /// Re-read everything about the tab being shown.
    /// Refresh the tab being shown.
    fn refresh_all(&mut self) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        self.refresh(&key);
    }

    fn set_theme(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        self.settings.theme = mode;
        self.palette = Palette::for_mode(mode);
        crate::ui::theme::set_mode(ctx, mode);
        // Diffs and blame carry baked-in syntax colours for the theme they were
        // built with, so they have to be recomputed. Cheap, and only on a toggle.
        self.current.repo.commit_diff = None;
        self.current.repo.staged = None;
        self.current.repo.unstaged = None;
        if let CenterView::Blame { path, at } = self.current.center.clone() {
            self.open_blame(path, at);
        }
        self.settings.save();
    }

    fn highlight_theme(&self) -> crate::git::highlight::HighlightTheme {
        use crate::git::highlight::HighlightTheme;
        if !self.settings.syntax_highlighting {
            return HighlightTheme::Off;
        }
        match self.settings.theme {
            ThemeMode::Dark => HighlightTheme::Dark,
            ThemeMode::Light => HighlightTheme::Light,
        }
    }

    fn pick_folder(&mut self) {
        self.pick_directory(PickerKind::OpenRepo, "Open Repository");
    }

    fn pick_folder_for_new_tab(&mut self) {
        self.pick_directory(PickerKind::NewTab, "Open Repository in a New Tab");
    }

    fn pick_clone_parent(&mut self) {
        self.pick_directory(PickerKind::CloneParent, "Clone Into");
    }

    fn pick_directory(&mut self, kind: PickerKind, title: &str) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        let tx = self.picker.0.clone();
        let title = title.to_owned();
        let start = self
            .current
            .repo
            .key
            .as_ref()
            .map(|k| k.0.clone())
            .or_else(|| std::env::current_dir().ok());
        // rfd blocks until the user answers, so it gets its own thread.
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            // Cancelling is an answer and has to be reported. Sending only on
            // success leaves `picker_open` set for good, and every later
            // attempt to open a repository is silently ignored.
            let _ = tx.send((kind, dialog.pick_folder()));
        });
    }

    fn start_clone(&mut self, url: String, parent: PathBuf, name: String) {
        self.jobs.dispatch(Job::Clone {
            url: url.trim().to_owned(),
            parent,
            name: name.trim().to_owned(),
        });
    }

    fn toast(&mut self, toast: Toast) {
        // Identical consecutive messages are noise, not information.
        if self.toasts.last().is_some_and(|t| t.text == toast.text) {
            return;
        }
        self.toasts.push(toast);
    }

    // ----------------------------------------------------------------- pumps

    fn pump_picker(&mut self) {
        while let Ok((kind, path)) = self.picker.1.try_recv() {
            self.finish_picker(kind, path);
        }
    }

    /// Act on the folder picker's answer, cancellation included.
    fn finish_picker(&mut self, kind: PickerKind, path: Option<PathBuf>) {
        self.picker_open = false;
        let Some(path) = path else {
            return;
        };
        {
            match kind {
                // Opening from the welcome screen fills the empty tab; opening
                // while a repository is showing gets its own tab, because
                // replacing what you were looking at is rarely what you meant.
                PickerKind::OpenRepo => {
                    if self.current.is_open() {
                        self.open_in_new_tab(path);
                    } else {
                        self.open_repo(path);
                    }
                }
                PickerKind::NewTab => self.open_in_new_tab(path),
                PickerKind::CloneParent => {
                    if let Some(crate::ui::dialog::Dialog::Clone { parent, .. }) =
                        self.dialog.as_mut()
                    {
                        *parent = path;
                    }
                }
            }
        }
    }

    fn pump_jobs(&mut self, ctx: &egui::Context) {
        for msg in self.jobs.poll() {
            match msg {
                Message::Done { id, outcome, .. } => {
                    self.clear_progress(id);
                    self.apply_outcome(ctx, outcome);
                }
                Message::Failed { id, topic, error } => {
                    self.clear_progress(id);
                    self.handle_error(topic.repo().cloned(), topic.session(), error);
                }
                Message::Progress {
                    id,
                    topic,
                    progress,
                } => {
                    self.progress.insert(topic.repo().cloned(), (id, progress));
                }
            }
        }
    }

    /// The session for a repository, whichever tab it is in.
    ///
    /// Results have to reach background tabs, not just the visible one: a fetch
    /// started in one tab must finish after the user moves to another, and a
    /// tab that quietly stopped updating while hidden would be worse than one
    /// that never loaded.
    fn session_for(&mut self, key: &RepoKey) -> Option<&mut Session> {
        if self.current.repo.key.as_ref() == Some(key) {
            return Some(&mut self.current);
        }
        self.background
            .iter_mut()
            .find(|s| s.repo.key.as_ref() == Some(key))
    }

    /// Whether `key` belongs to the tab currently on screen.
    fn is_current(&self, key: &RepoKey) -> bool {
        self.current.repo.key.as_ref() == Some(key)
    }

    fn apply_outcome(&mut self, ctx: &egui::Context, outcome: Outcome) {
        match outcome {
            Outcome::RepoOpened {
                token,
                key,
                head,
                git_dir,
            } => {
                // The session that asked may have been closed, or replaced by
                // a later open, while this was in flight.
                if self.session_by_id(token).is_none() {
                    return;
                }
                self.settings.touch_recent(&key.0);

                // The same repository already open elsewhere should not get a
                // second tab; drop this one and switch to the existing tab.
                let already_open = std::iter::once(&self.current)
                    .chain(self.background.iter())
                    .any(|s| s.id != token && s.repo.key.as_ref() == Some(&key));
                if already_open {
                    self.discard_session(token);
                    self.activate_by_key(&key);
                    return;
                }

                let watcher = self.build_watcher(ctx, &key, &git_dir);
                if let Some(session) = self.session_by_id(token) {
                    session.repo.loading = false;
                    session.git_dir = Some(git_dir);
                    session.repo.key = Some(key.clone());
                    session.repo.head = Some(*head);
                    session.watcher = watcher;
                }

                // Recorded only now: the tab has to have its key before it can
                // be remembered.
                self.remember_open_tabs();
                self.load_repository(&key);
            }

            Outcome::Head { key, head } => {
                if let Some(session) = self.session_for(&key) {
                    session.repo.head = Some(*head);
                }
            }

            Outcome::Status { key, status } => {
                let conflicted = status.conflict_count > 0;
                let Some(session) = self.session_for(&key) else {
                    return;
                };
                session.repo.status = Some(status);
                session.ensure_selection();
                if !conflicted {
                    session.conflicts = None;
                    session.conflict_file = None;
                    session.conflict_buffer.clear();
                    // Nothing left to resolve, so stop showing a view that
                    // would be permanently empty.
                    if session.workdir_view == WorkdirView::Conflicts {
                        session.workdir_view = WorkdirView::Unstaged;
                    }
                }
                if conflicted {
                    self.jobs.dispatch(Job::LoadConflicts(key));
                }
            }

            Outcome::Mutated {
                key,
                message,
                moved_head,
            } => {
                let visible = self.is_current(&key);
                let Some(session) = self.session_for(&key) else {
                    return;
                };
                // Refresh explicitly rather than waiting for the watcher: it
                // may be off, and even when on, a visible lag between clicking
                // Stage and the list updating feels broken.
                session.repo.staged = None;
                session.repo.unstaged = None;
                session.line_selection.clear();
                if moved_head {
                    session.repo.commit_diff = None;
                    session.commit_message.clear();
                    session.message_is_draft = false;
                    session.amending = false;
                }

                self.pending_delete = None;
                // A message about a tab you are not looking at would be
                // confusing; the tab's own badge reports it instead.
                if visible && !message.is_empty() {
                    self.toast(Toast::success(message));
                }
                self.refresh(&key);
            }

            Outcome::Conflicts { key, conflicts } => {
                let visible = self.is_current(&key);
                let Some(session) = self.session_for(&key) else {
                    return;
                };
                // Landing in a conflicted state should show the conflicts, not
                // leave the user to find them — but only in the tab being
                // looked at, since switching a hidden tab's view is invisible
                // and surprising when the user returns.
                if visible && !conflicts.is_empty() && session.conflicts.is_none() {
                    session.workdir_view = WorkdirView::Conflicts;
                    session.repo.selection = Some(Selection::Workdir);
                    session.repo.selection_pinned = true;
                }
                if session
                    .conflict_file
                    .as_ref()
                    .is_none_or(|p| conflicts.find(p).is_none())
                {
                    session.conflict_file = conflicts.files.first().map(|c| c.path.clone());
                    session.conflict_buffer.clear();
                }
                session.conflicts = Some(conflicts);
            }

            Outcome::Identity { key, identities } => {
                // Only for the repository the sheet is showing. A background
                // tab finishing its own read must not swap the fields out from
                // under whoever is typing.
                let current = self.current.repo.key.clone();
                if key != current && key.is_some() {
                    return;
                }
                self.identity = Some(*identities);
                self.reseed_identity_draft();
            }

            Outcome::Submodules { key, submodules } => {
                if let Some(session) = self.session_for(&key) {
                    session.submodules = Some(submodules);
                }
            }

            Outcome::Blame { key, result } => {
                if let Some(session) = self.session_for(&key) {
                    session.blame = Some(result);
                }
            }

            Outcome::Search {
                key,
                results,
                kind: _,
            } => {
                let empty = results.commits.is_empty() && !results.query.is_empty();
                let query = results.query.clone();
                let visible = self.is_current(&key);
                if let Some(session) = self.session_for(&key) {
                    session.search_results = Some(results);
                }
                if empty && visible {
                    self.toast(Toast::info(format!("No commits match ‘{query}’")));
                }
            }

            Outcome::FileHistory { key, results, path } => {
                let empty = results.commits.is_empty();
                let visible = self.is_current(&key);
                if let Some(session) = self.session_for(&key) {
                    if empty {
                        session.center = CenterView::History;
                    }
                    session.search_results = Some(results);
                }
                if empty && visible {
                    self.toast(Toast::info(format!("No history for {path}")));
                }
            }

            Outcome::Cloned { path } => {
                self.toast(Toast::success(format!(
                    "Cloned into {}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                )));
                // A clone is a new repository, not a replacement for whatever
                // you were looking at.
                self.open_in_new_tab(path);
            }

            Outcome::Refs { key, tree } => {
                if let Some(session) = self.session_for(&key) {
                    session.repo.refs = Some(tree);
                }
            }

            Outcome::Diff { key, target, model } => {
                let Some(session) = self.session_for(&key) else {
                    return;
                };
                match target {
                    crate::git::DiffTarget::Commit(oid) => {
                        session.repo.commit_diff = Some((oid, model));
                    }
                    crate::git::DiffTarget::Staged => session.repo.staged = Some(model),
                    crate::git::DiffTarget::Unstaged => session.repo.unstaged = Some(model),
                }
                session.ensure_active_file();
            }

            Outcome::Graph { key, page, limit } => {
                let Some(session) = self.session_for(&key) else {
                    return;
                };
                // A rewrite done elsewhere — a rebase or an amend in a
                // terminal — can leave the selection pointing at a commit
                // that no longer exists. Unpin it so the app can choose
                // again, rather than showing a detail pane that never loads.
                //
                // Only while looking at the graph: search results and file
                // history legitimately name commits outside it, and
                // resetting there would throw away what the user found.
                if session.center.is_history() {
                    if let Some(Selection::Commit(oid)) = session.repo.selection {
                        if !page.rows.iter().any(|r| r.commit.id == oid) {
                            session.repo.selection_pinned = false;
                            session.repo.selection = None;
                        }
                    }
                }
                session.repo.graph = Some(page);
                session.repo.graph_limit = limit;
                session.ensure_selection();
            }
        }
    }

    /// Forget any progress recorded for a finished job.
    fn clear_progress(&mut self, id: u64) {
        self.progress.retain(|_, (job, _)| *job != id);
    }

    /// How this platform writes the chord bound to `action`, or nothing when
    /// the user has unbound it.
    ///
    /// Every shortcut the interface advertises goes through here. Writing them
    /// out as literals meant they were both macOS-only and free to drift from
    /// the keymap — the theme button spent a while advertising Pull's chord.
    fn shortcut(&self, action: A) -> Option<String> {
        self.settings.keymap.chord(action).map(|c| c.display())
    }

    /// `label` with its chord appended, for a tooltip.
    fn hint(&self, label: &str, action: A) -> String {
        match self.shortcut(action) {
            Some(chord) => format!("{label}  {chord}"),
            None => label.to_owned(),
        }
    }

    /// Progress worth showing in the status bar: this tab's, or a clone's.
    fn visible_progress(&self) -> Option<crate::job::Progress> {
        let own = self
            .current
            .repo
            .key
            .clone()
            .and_then(|key| self.progress.get(&Some(key)).cloned());
        own.or_else(|| self.progress.get(&None).cloned())
            .map(|(_, progress)| progress)
    }

    /// Whether the repository in the tab being shown has an operation running.
    fn current_is_busy(&self) -> bool {
        self.current
            .repo
            .key
            .clone()
            .is_some_and(|key| self.progress.contains_key(&Some(key)))
    }

    /// Whether `key` has an operation running.
    fn repo_is_busy(&self, key: &RepoKey) -> bool {
        self.progress.contains_key(&Some(key.clone()))
    }

    /// Record a selection the user made explicitly.
    fn select(&mut self, selection: Selection) {
        self.current.repo.selection = Some(selection);
        self.current.repo.selection_pinned = true;
    }

    /// Request whatever diffs the current selection needs and doesn't have.
    ///
    /// Called every frame; it is cheap because `required_diffs` returns nothing
    /// once the data is present, and the job system supersedes duplicates.
    fn request_diffs(&mut self) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        for target in self.current.repo.required_diffs() {
            let job = Job::LoadDiff {
                repo: key.clone(),
                target,
                theme: self.highlight_theme(),
            };
            // Asking the job system, rather than keeping a record here, is what
            // makes this correct across supersession: a request that got
            // cancelled is simply no longer pending.
            if self.jobs.is_pending(&job) {
                continue;
            }
            self.jobs.dispatch(job);
        }
    }

    /// Ask for a longer walk, once, when the user scrolls near the end.
    fn grow_graph(&mut self) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        // Grow steeply: each growth re-walks the whole history, so doubling
        // would pay that cost far too often on a large repository.
        let more = self
            .current
            .repo
            .graph_limit
            .saturating_mul(8)
            .max(INITIAL_GRAPH_LIMIT);
        let job = Job::LoadGraph {
            repo: key,
            limit: more,
        };
        // Same reasoning as `request_diffs`: a flag set here would survive a
        // failed or superseded walk and block growth for good, because nothing
        // would ever arrive to clear it.
        if self.jobs.is_pending(&job) {
            return;
        }
        self.jobs.dispatch(job);
    }

    /// A watcher for a repository, or `None` when watching is off or failed.
    ///
    /// Returns rather than assigns, so the caller can put it on whichever
    /// session the result belongs to — which is not always the current one.
    fn build_watcher(
        &mut self,
        ctx: &egui::Context,
        key: &RepoKey,
        git_dir: &std::path::Path,
    ) -> Option<RepoWatcher> {
        if !self.settings.auto_refresh {
            return None;
        }
        match RepoWatcher::new(&key.0, git_dir, ctx.clone()) {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                // Losing the watcher degrades to manual refresh; not fatal.
                self.toast(Toast::error(e.user_message()));
                None
            }
        }
    }

    /// Report a failure, attributing it to the repository it came from.
    fn handle_error(&mut self, repo: Option<RepoKey>, session: Option<u64>, error: Error) {
        let message = error.user_message();

        // Which tab this actually happened in. A failed open has no repository
        // key yet, so it names its tab instead; clearing the visible tab's
        // `loading` regardless would leave the failed tab skeletal for good
        // while blanking a tab that was loading perfectly well.
        // A failure in a tab the user is not looking at should not interrupt
        // them with a message about a repository they cannot see. Decided
        // before anything below moves tabs around, so a failed open still
        // reports itself in the tab the user was watching.
        let visible = match (&repo, session) {
            (Some(key), _) => self.is_current(key),
            (None, Some(id)) => self.current.id == id,
            (None, None) => true,
        };

        let blamed = match repo.as_ref().and_then(|key| self.session_for(key)) {
            Some(found) => Some((found.id, found.repo.key.is_none())),
            None => session
                .and_then(|id| self.session_by_id(id))
                .map(|found| (found.id, found.repo.key.is_none())),
        };

        match blamed {
            // A tab that never got a repository is nothing but the failure.
            // Leaving it would strand an empty tab the user has to close.
            Some((id, true)) => self.discard_session(id),
            Some((id, false)) => {
                if let Some(found) = self.session_by_id(id) {
                    found.repo.loading = false;
                }
            }
            // Untraceable: a clone, or a tab that has since been closed.
            None => self.current.repo.loading = false,
        }

        // A branch delete refused for unmerged commits is not really a failure:
        // it is a question. Re-open the dialog with the answer available.
        if let Some(name) = self.pending_delete.take() {
            if message.contains("aren't merged") {
                self.dialog = Some(crate::ui::dialog::Dialog::DeleteBranch {
                    name,
                    force: true,
                    warning: Some(message),
                });
                return;
            }
        }

        if visible {
            self.toast(Toast::error(message));
        } else {
            tracing::warn!("background tab: {message}");
        }
    }

    fn pump_watcher(&mut self) {
        // Every tab is watched, not just the visible one. A background tab that
        // stopped noticing commits would show a stale badge, which is worse
        // than no badge at all — and the whole reason to keep it open is to
        // glance at it.
        let mut changed: Vec<(RepoKey, crate::watch::Change)> = Vec::new();
        for session in std::iter::once(&self.current).chain(self.background.iter()) {
            let (Some(watcher), Some(key)) = (&session.watcher, &session.repo.key) else {
                continue;
            };
            if let Some(change) = watcher.poll() {
                changed.push((key.clone(), change));
            }
        }

        for (key, change) in changed {
            if change.refs {
                let limit = self
                    .session_for(&key)
                    .map(|s| s.repo.graph_limit.max(INITIAL_GRAPH_LIMIT))
                    .unwrap_or(INITIAL_GRAPH_LIMIT);
                self.jobs.dispatch(Job::ReadHead(key.clone()));
                self.jobs.dispatch(Job::LoadGraph {
                    repo: key.clone(),
                    limit,
                });
                self.jobs.dispatch(Job::LoadRefs(key.clone()));
                self.jobs.dispatch(Job::LoadSubmodules(key.clone()));
            }
            if change.worktree || change.index || change.refs {
                if let Some(session) = self.session_for(&key) {
                    session.repo.unstaged = None;
                    session.repo.staged = None;
                }
                self.jobs.dispatch(Job::ReadStatus {
                    repo: key,
                    include_ignored: self.settings.show_ignored,
                });
            }
        }
    }

    fn expire_toasts(&mut self) {
        self.toasts.retain(|t| !t.is_expired());
    }

    // ------------------------------------------------------------------ input

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};

        // A modal owns the keyboard while it is up. This has to come first,
        // including before Escape: `consume_key` removes the event, so handling
        // Escape here would swallow the very key the dialog needs to close on —
        // and the dialogs are drawn after this runs.
        if self.palette_open || self.settings_open || self.dialog.is_some() {
            return;
        }

        // Tab switching is positional, so it is spelled out rather than bound:
        // ⌘1…⌘9 are what every tabbed application uses, and remapping nine of
        // them individually would be a chore nobody wants.
        for (offset, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
        ]
        .into_iter()
        .enumerate()
        {
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key)) {
                self.activate(offset);
            }
        }
        // ⌘9 goes to the last tab, matching browsers rather than counting.
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Num9)) {
            self.activate(self.tab_count().saturating_sub(1));
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::T)) {
            self.pick_folder_for_new_tab();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::W)) && self.current.is_open() {
            self.close_tab(self.current_slot);
        }
        // Cycling, for when you do not want to count either.
        if ctx
            .input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::CloseBracket))
        {
            self.cycle_tab(1);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::OpenBracket))
        {
            self.cycle_tab(-1);
        }

        // Escape is not remappable. It means "back out of this" everywhere on
        // the system, and its meaning here depends on what is open — which is
        // not something a binding table can express.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if !self.toasts.is_empty() {
                self.toasts.clear();
            } else if !self.current.center.is_history() {
                self.leave_center_view();
            }
        }

        if let Some(action) = self
            .settings
            .keymap
            .triggered(ctx, self.current.repo.is_open())
        {
            self.run_action(ctx, action);
        }
    }

    /// Perform a bound action.
    fn run_action(&mut self, ctx: &egui::Context, action: crate::ui::keymap::Action) {
        use crate::ui::command::Command;
        use crate::ui::keymap::Action as A;

        match action {
            A::CommandPalette => self.open_palette(),
            A::OpenRepository => self.pick_folder(),
            A::Settings => self.open_settings(),
            A::Refresh => self.refresh_all(),
            A::Search => self.search_focus = true,
            A::ToggleTheme => {
                let next = self.settings.theme.toggled();
                self.set_theme(ctx, next);
            }
            // A binding that does nothing is worse than one that says why.
            A::DraftMessage => self.draft_message(),
            A::Fetch => match self.fetch_blocker() {
                Some(reason) => self.toast(Toast::info(reason)),
                None => self.run_command(ctx, Command::Fetch),
            },
            A::Pull => match self.pull_blocker() {
                Some(reason) => self.toast(Toast::info(reason)),
                None => self.run_command(ctx, Command::Pull),
            },
            A::Push => match self.push_blocker() {
                Some(reason) => self.toast(Toast::info(reason)),
                None => self.run_command(ctx, Command::Push),
            },
            A::StageAll => self.run_command(ctx, Command::StageAll),
            A::Stash => self.run_command(ctx, Command::StashChanges),
            A::NewBranch => self.run_command(ctx, Command::NewBranch),
            A::Commit => self.do_commit(),
        }
    }
}

impl GitupApp {
    /// Advance state. Called once per frame before drawing; also callable
    /// directly by tests.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.flush_layout(ctx);
        self.pump_dropped_files(ctx);
        self.pump_picker();
        self.pump_jobs(ctx);
        self.pump_watcher();
        self.request_diffs();
        self.expire_toasts();
        self.sync_window_title(ctx);

        // Toasts fade on a timer, so keep frames coming while any are showing.
        if !self.toasts.is_empty() || self.jobs.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Record a layout size the user dragged to, for writing out later.
    ///
    /// Sizes arrive every frame whether or not anything moved, so this only
    /// marks the settings dirty when the value actually changed — otherwise
    /// the file would be rewritten after every mouse-up anywhere.
    fn remember_layout(&mut self, field: impl Fn(&mut Settings) -> &mut f32, size: f32) {
        if !self.dragging {
            return;
        }
        let slot = field(&mut self.settings);
        // Sub-pixel drift from layout rounding is not the user moving anything.
        if (*slot - size).abs() > 0.5 {
            *slot = size;
            self.layout_dirty = true;
        }
    }

    /// What the detail pane spends on furniture rather than on diff.
    ///
    /// Deliberately not conditional on what is selected, even though the commit
    /// box only appears for the working tree. `Panel` takes its default size
    /// from the first frame and ignores the argument afterwards — and on the
    /// first frame no repository has loaded, so nothing is selected yet. A
    /// furniture figure that depended on the selection was therefore always
    /// computed in the one state where the answer is wrong, and the panel kept
    /// that answer for the rest of the session.
    ///
    /// The commit box height is read from settings rather than assumed, so the
    /// split stays right for someone who has dragged it taller.
    fn detail_furniture(&self) -> f32 {
        metrics::DETAIL_HEADERS + self.settings.commit_box_height
    }

    /// As [`remember_layout`](Self::remember_layout), for the detail pane's
    /// share of the centre rather than a pixel height.
    fn remember_share(&mut self, share: f32) {
        if !self.dragging {
            return;
        }
        if (self.settings.detail_share - share).abs() > 0.002 {
            self.settings.detail_share = share;
            self.layout_dirty = true;
        }
    }

    /// Write out dragged layout sizes, once the drag is over.
    fn flush_layout(&mut self, ctx: &egui::Context) {
        self.dragging = ctx.input(|i| i.pointer.any_down());
        if !self.layout_dirty || self.dragging {
            return;
        }
        self.layout_dirty = false;
        self.settings.save();
    }

    /// Open a repository dropped onto the window.
    ///
    /// Dragging a folder onto a Git client is how people expect to open one,
    /// and nothing here answered a drop at all. A file is accepted as well as
    /// a directory, because dropping a file from a repository plainly means
    /// that repository — refusing it on a technicality helps nobody.
    fn pump_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }

        let mut opened = 0;
        for path in dropped {
            // Every repository gets its own tab, in drop order, the same as
            // choosing several from the picker would.
            let directory = if path.is_dir() {
                path
            } else {
                match path.parent() {
                    Some(parent) => parent.to_path_buf(),
                    None => continue,
                }
            };
            if crate::git::repo::discover(&directory).is_err() {
                self.toast(Toast::error(format!(
                    "{} is not inside a Git repository",
                    directory.display()
                )));
                continue;
            }
            if opened == 0 && !self.current.is_open() && !self.current.repo.loading {
                self.open_repo(directory);
            } else {
                self.open_in_new_tab(directory);
            }
            opened += 1;
        }
    }

    /// Whether a drag is currently over the window, so the UI can say it will
    /// be caught.
    fn is_hovering_files(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| !i.raw.hovered_files.is_empty())
    }

    /// Keep the window title on the repository being shown.
    ///
    /// With several open, the title is the only thing identifying the window
    /// in a task switcher or a window list.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.current.repo.key.as_ref() {
            Some(_) => {
                let name = self.current.title();
                match self.current.repo.head.as_ref() {
                    Some(head) => format!("{name} — {} — {APP_NAME}", head.display_name()),
                    None => format!("{name} — {APP_NAME}"),
                }
            }
            None => APP_NAME.to_owned(),
        };
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// Draw one frame.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.handle_shortcuts(ui.ctx());

        self.draw_toolbar(ui);
        self.draw_tab_bar(ui);
        self.draw_operation_banner(ui);
        self.draw_statusbar(ui);
        if self.current.repo.is_open() {
            self.draw_sidebar(ui);
            self.draw_center(ui);
        } else {
            self.draw_welcome(ui);
        }
        self.draw_settings(ui.ctx());
        self.draw_palette(ui.ctx());
        self.draw_dialog(ui.ctx());
        self.draw_toasts(ui.ctx());
        self.draw_drop_target(ui.ctx());
    }

    fn draw_settings(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        // Read the git version lazily: it costs a process spawn, and most
        // sessions never open this sheet.
        if self.identity.is_none() {
            self.load_identity();
        }
        if self.git_version.is_none() {
            self.git_version = crate::git::cli::git_command()
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_owned());
        }

        let mut settings = self.settings.clone();
        let mut recording = self.recording_binding;
        let mut draft = self.identity_draft.clone();
        let mut scope = self.identity_scope;
        let dirty = self.identity_is_dirty();
        let mut sheet = crate::ui::settings_panel::SettingsSheet {
            palette: &self.palette,
            settings: &mut settings,
            git_version: self.git_version.as_deref(),
            recording: &mut recording,
            identity: self.identity.as_ref(),
            identity_draft: &mut draft,
            identity_scope: &mut scope,
            has_repository: self.current.repo.key.is_some(),
            identity_dirty: dirty,
        };
        let response = sheet.show(ctx);
        self.settings = settings;
        self.recording_binding = recording;
        self.identity_draft = draft;
        self.identity_scope = scope;

        if response.identity_scope_changed {
            self.reseed_identity_draft();
        }
        if response.save_identity {
            self.save_identity();
        }

        if response.theme_changed {
            let mode = self.settings.theme;
            self.set_theme(ctx, mode);
        }
        if response.diffs_invalidated {
            self.current.repo.commit_diff = None;
            self.current.repo.staged = None;
            self.current.repo.unstaged = None;
        }
        if response.watcher_changed {
            if self.settings.auto_refresh {
                self.restart_watchers(ctx);
            } else {
                self.stop_watchers();
            }
        }
        if response.reload {
            self.refresh_all();
        }
        self.settings.save();

        if response.close {
            self.settings_open = false;
            self.recording_binding = None;
        }
    }

    /// Say that a dragged folder will be caught, while it is over the window.
    ///
    /// A drop that works but gives no sign it will is indistinguishable from
    /// one that does not, so people let go somewhere else and conclude the
    /// application cannot do it.
    fn draw_drop_target(&mut self, ctx: &egui::Context) {
        if !self.is_hovering_files(ctx) {
            return;
        }
        let p = self.palette;
        let screen = ctx.content_rect();

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_target"),
        ));
        painter.rect_filled(screen, CornerRadius::ZERO, p.bg_base.gamma_multiply(0.82));
        let inset = screen.shrink(space::XL);
        painter.rect_stroke(
            inset,
            CornerRadius::same(radius::LG),
            Stroke::new(2.0, p.accent),
            egui::StrokeKind::Inside,
        );
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to open",
            egui::FontId::new(text::size::TITLE, egui::FontFamily::Proportional),
            p.text,
        );
    }

    fn draw_palette(&mut self, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }
        let entries = self.palette_entries();
        let mut query = std::mem::take(&mut self.palette_query);
        let mut selected = self.palette_selected;

        let mut palette = crate::ui::command::CommandPalette {
            palette: &self.palette,
            query: &mut query,
            selected: &mut selected,
            entries: &entries,
            just_opened: self.palette_just_opened,
        };
        let response = palette.show(ctx);

        self.palette_query = query;
        self.palette_selected = selected;
        self.palette_just_opened = false;

        if let Some(command) = response.chosen {
            self.close_palette();
            self.run_command(ctx, command);
        } else if response.dismissed {
            self.close_palette();
        }
    }

    /// Titles of the open tabs, in bar order. For tests.
    pub fn tab_titles_for_test(&self) -> Vec<String> {
        self.tab_order()
            .iter()
            .map(|tab| self.session_at(*tab).title())
            .collect()
    }

    /// Which tab is showing. For tests.
    pub fn active_tab_for_test(&self) -> usize {
        self.current_slot
    }

    /// The HEAD summary of the tab at `position`, once loaded. For tests.
    pub fn tab_head_for_test(&self, position: usize) -> Option<String> {
        let tab = *self.tab_order().get(position)?;
        self.session_at(tab)
            .repo
            .head
            .as_ref()
            .map(|h| h.summary.clone())
    }

    /// How many files the tab at `position` reports as changed. For tests.
    pub fn tab_change_count_for_test(&self, position: usize) -> Option<usize> {
        let tab = *self.tab_order().get(position)?;
        self.session_at(tab)
            .repo
            .status
            .as_ref()
            .map(|s| s.entries.len())
    }

    /// Re-read the repository in the tab at `position`. For tests.
    pub fn refresh_tab_for_test(&mut self, position: usize) {
        let Some(&tab) = self.tab_order().get(position) else {
            return;
        };
        let Some(key) = self.session_at(tab).repo.key.clone() else {
            return;
        };
        self.refresh(&key);
    }

    /// The settings as they stand. For tests.
    pub fn settings_for_test(&self) -> &Settings {
        &self.settings
    }

    /// Show a message, as a job failure would. For tests.
    pub fn push_toast_for_test(&mut self, text: &str) {
        self.toast(Toast::error(text.to_owned()));
    }

    /// Pretend the folder picker was opened and answered. For tests, which
    /// cannot drive a native dialog.
    pub fn simulate_pick_for_test(&mut self, path: Option<PathBuf>) {
        self.picker_open = true;
        self.finish_picker(PickerKind::OpenRepo, path);
    }

    /// Whether a folder picker is believed to be open. For tests.
    pub fn picker_is_open_for_test(&self) -> bool {
        self.picker_open
    }

    /// Pixels the diff body last had to render lines in. For tests.
    pub fn diff_body_height_for_test(&self) -> f32 {
        self.diff_body_height
    }

    /// The file selected in the changed-file list. For tests.
    pub fn active_file_for_test(&self) -> Option<&str> {
        self.current.repo.active_file.as_deref()
    }

    /// What is selected in history. For tests.
    pub fn selection_for_test(&self) -> Option<Selection> {
        self.current.repo.selection
    }

    /// Whether the arrow keys are moving through history. For tests.
    pub fn history_has_focus_for_test(&self) -> bool {
        self.current.focus == crate::state::Focus::History
    }

    /// How many files are staged. For tests.
    pub fn staged_count_for_test(&self) -> usize {
        self.current
            .repo
            .status
            .as_ref()
            .map(|s| s.staged_count)
            .unwrap_or(0)
    }

    /// The identity as last read. For tests.
    pub fn identity_for_test(&self) -> Option<&crate::git::identity::Identities> {
        self.identity.as_ref()
    }

    /// Ask for the identity, as opening the settings sheet does. For tests.
    pub fn load_identity_for_test(&mut self) {
        self.load_identity();
    }

    /// Fill in the identity fields and save them. For tests.
    pub fn set_identity_for_test(
        &mut self,
        scope: crate::git::identity::Scope,
        name: &str,
        email: &str,
    ) {
        self.identity_scope = scope;
        self.identity_draft = crate::git::identity::Identity {
            name: name.to_owned(),
            email: email.to_owned(),
        };
        self.save_identity();
    }

    /// Whether the identity fields differ from what is stored. For tests.
    pub fn identity_is_dirty_for_test(&self) -> bool {
        self.identity_is_dirty()
    }

    /// The text in the commit message box. For tests.
    pub fn commit_message_for_test(&self) -> &str {
        &self.current.commit_message
    }

    /// Type into the commit message box, as the user would. For tests.
    pub fn type_commit_message_for_test(&mut self, text: &str) {
        self.current.commit_message = text.to_owned();
        self.current.message_is_draft = false;
    }

    /// Run the draft action. For tests.
    pub fn draft_message_for_test(&mut self) {
        self.draft_message();
    }

    /// Whether the draft button would be offered. For tests.
    pub fn can_draft_message_for_test(&self) -> bool {
        self.can_draft_message()
    }

    /// The commit whose diff is loaded in the current tab. For tests.
    pub fn loaded_commit_diff_for_test(&self) -> Option<git2::Oid> {
        self.current.repo.commit_diff.as_ref().map(|(oid, _)| *oid)
    }

    /// Advance one frame without drawing. For tests.
    pub fn tick_for_test(&mut self, ctx: &egui::Context) {
        self.tick(ctx);
    }

    /// Pretend an operation is running in the tab at `position`. For tests.
    pub fn set_progress_for_test(&mut self, position: usize, label: &str) {
        let Some(&tab) = self.tab_order().get(position) else {
            return;
        };
        let Some(key) = self.session_at(tab).repo.key.clone() else {
            return;
        };
        self.progress.insert(
            Some(key),
            (
                u64::MAX,
                crate::job::Progress {
                    label: label.to_owned(),
                    done: 1,
                    total: Some(2),
                },
            ),
        );
    }

    /// Whether the tab at `position` has an operation running. For tests.
    pub fn tab_is_busy_for_test(&self, position: usize) -> bool {
        self.tab_order()
            .get(position)
            .and_then(|tab| self.session_at(*tab).repo.key.as_ref())
            .is_some_and(|key| self.repo_is_busy(key))
    }

    /// Whether the tab being shown has an operation running. For tests.
    pub fn current_is_busy_for_test(&self) -> bool {
        self.current_is_busy()
    }

    /// Whether the tab at `position` has its history loaded. For tests.
    pub fn tab_has_graph_for_test(&self, position: usize) -> bool {
        self.tab_order()
            .get(position)
            .map(|tab| self.session_at(*tab).repo.graph.is_some())
            .unwrap_or(false)
    }

    /// Whether the tab at `position` has its branches loaded. For tests.
    pub fn tab_has_refs_for_test(&self, position: usize) -> bool {
        self.tab_order()
            .get(position)
            .map(|tab| self.session_at(*tab).repo.refs.is_some())
            .unwrap_or(false)
    }

    /// Whether the tab at `position` has its status loaded. For tests.
    pub fn tab_has_status_for_test(&self, position: usize) -> bool {
        self.tab_order()
            .get(position)
            .map(|tab| self.session_at(*tab).repo.status.is_some())
            .unwrap_or(false)
    }

    /// Why pulling is unavailable, if it is. For tests.
    pub fn pull_blocker_for_test(&self) -> Option<String> {
        self.pull_blocker()
    }

    /// Run a palette command directly. For tests.
    pub fn run_command_for_test(
        &mut self,
        ctx: &egui::Context,
        command: crate::ui::command::Command,
    ) {
        self.run_command(ctx, command);
    }

    /// Whether any job is still running. For tests.
    pub fn is_busy_for_test(&self) -> bool {
        self.jobs.is_busy()
    }

    /// Close the tab at `position`. For tests.
    pub fn close_tab_for_test(&mut self, position: usize) {
        self.close_tab(position);
    }

    /// Select a commit in the current tab. For tests.
    pub fn select_commit_for_test(&mut self, oid: git2::Oid) {
        self.select(Selection::Commit(oid));
    }

    /// The commit selected in the tab at `position`. For tests.
    pub fn tab_selection_for_test(&self, position: usize) -> Option<git2::Oid> {
        let tab = *self.tab_order().get(position)?;
        self.session_at(tab).repo.selected_commit()
    }

    /// Open a repository in another tab. For tests, which cannot drive the
    /// folder picker.
    pub fn open_in_new_tab_for_test(&mut self, path: PathBuf) {
        self.open_in_new_tab(path);
    }

    /// Show the tab at `position`. For tests.
    pub fn activate_for_test(&mut self, position: usize) {
        self.activate(position);
    }

    /// Open the settings sheet. For tests.
    pub fn open_settings_for_test(&mut self) {
        self.open_settings();
        // Pinned so the sheet's About section doesn't vary by machine.
        self.git_version = Some("git version 2.50.0".to_owned());
        // Likewise the identity — and this one matters more than determinism.
        // Reading the real global config would render whoever ran the tests
        // into a committed PNG, and the snapshots are published.
        self.identity = Some(crate::git::identity::Identities {
            global: crate::git::identity::Identity {
                name: "Ada Lovelace".to_owned(),
                email: "ada@example.com".to_owned(),
            },
            repository: crate::git::identity::Identity::default(),
            effective: crate::git::identity::Identity {
                name: "Ada Lovelace".to_owned(),
                email: "ada@example.com".to_owned(),
            },
        });
        self.reseed_identity_draft();
    }

    /// Choose the diff layout directly. For tests.
    pub fn set_diff_layout_for_test(&mut self, layout: crate::ui::diff::DiffLayout) {
        self.settings.diff_layout = layout;
    }

    /// Open the palette with a query already typed. For tests, which cannot
    /// send a key chord to a modal that isn't open yet.
    pub fn open_palette_for_test(&mut self, query: &str) {
        self.open_palette();
        self.palette_query = query.to_owned();
    }

    fn open_palette(&mut self) {
        self.palette_open = true;
        self.palette_just_opened = true;
        self.palette_query.clear();
        self.palette_selected = 0;
    }

    fn close_palette(&mut self) {
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
    }

    /// Everything the palette can offer, given the current state.
    ///
    /// Rebuilt each frame the palette is open. That keeps it honest — a branch
    /// that was just deleted is not offered — and costs nothing, because it
    /// only runs while the palette is showing.
    fn palette_entries(&self) -> Vec<crate::ui::command::Entry> {
        use crate::ui::command::{Command, Entry};
        let mut entries = Vec::new();

        entries.push(
            Entry::new(
                Command::OpenRepository,
                "Open repository…",
                icons::FOLDER_OPEN,
            )
            .shortcut_opt(self.shortcut(A::OpenRepository)),
        );
        if self.current.is_open() {
            entries.push(
                Entry::new(Command::CloseTab, "Close this tab", icons::X)
                    .shortcut(Chord::cmd(egui::Key::W).display()),
            );
            // Switching by name beats counting tabs once there are more than a
            // couple, and the palette already matches on names.
            for (position, tab) in self.tab_order().iter().enumerate() {
                if *tab == TabRef::Current {
                    continue;
                }
                let session = self.session_at(*tab);
                entries.push(
                    Entry::new(
                        Command::SwitchTab(position),
                        format!("Switch to {}", session.title()),
                        icons::FOLDER_OPEN,
                    )
                    .detail(
                        session
                            .repo
                            .head
                            .as_ref()
                            .map(|h| h.display_name())
                            .unwrap_or_default(),
                    )
                    .weight(8),
                );
            }
        }
        entries.push(Entry::new(
            Command::CloneRepository,
            "Clone repository…",
            icons::DOWNLOAD_SIMPLE,
        ));
        entries.push(
            Entry::new(Command::ToggleTheme, "Toggle light/dark theme", icons::SUN)
                .shortcut_opt(self.shortcut(A::ToggleTheme)),
        );

        if !self.current.repo.is_open() {
            return entries;
        }

        let status = self.current.repo.status.clone().unwrap_or_default();
        let head = self.current.repo.head.as_ref();
        let on_branch = matches!(head.map(|h| &h.kind), Some(HeadKind::Branch(_)));

        entries.push(
            Entry::new(Command::Refresh, "Refresh", icons::ARROW_CLOCKWISE)
                .shortcut_opt(self.shortcut(A::Refresh)),
        );
        entries.push(
            Entry::new(Command::Fetch, "Fetch all remotes", icons::ARROWS_CLOCKWISE)
                .shortcut_opt(self.shortcut(A::Fetch))
                .weight(20),
        );
        if self.pull_blocker().is_none() {
            entries.push(
                Entry::new(Command::Pull, "Pull", icons::CLOUD_ARROW_DOWN)
                    .shortcut_opt(self.shortcut(A::Pull))
                    .weight(20),
            );
            entries.push(Entry::new(
                Command::PullRebase,
                "Pull with rebase",
                icons::CLOUD_ARROW_DOWN,
            ));
        }
        if self.push_blocker().is_none() {
            entries.push(
                Entry::new(Command::Push, "Push…", icons::CLOUD_ARROW_UP)
                    .shortcut_opt(self.shortcut(A::Push))
                    .weight(20),
            );
        }
        let _ = on_branch;

        if status.unstaged_count > 0 || status.untracked_count > 0 {
            entries
                .push(Entry::new(Command::StageAll, "Stage all changes", icons::PLUS).weight(15));
        }
        if status.staged_count > 0 {
            entries.push(Entry::new(
                Command::UnstageAll,
                "Unstage everything",
                icons::MINUS,
            ));
            entries.push(
                Entry::new(Command::FocusCommitMessage, "Commit…", icons::GIT_COMMIT).weight(25),
            );
            entries.push(
                Entry::new(
                    Command::DraftMessage,
                    "Draft a commit message",
                    icons::MAGIC_WAND,
                )
                .shortcut_opt(self.shortcut(A::DraftMessage))
                .weight(24),
            );
        }
        if !self.current.repo.head.as_ref().is_some_and(|h| h.is_empty) {
            entries.push(Entry::new(
                Command::AmendLast,
                "Amend the last commit",
                icons::PENCIL_SIMPLE,
            ));
        }
        if !status.is_clean() {
            entries.push(Entry::new(
                Command::StashChanges,
                "Stash changes…",
                icons::ARCHIVE,
            ));
        }

        entries.push(Entry::new(Command::NewBranch, "New branch…", icons::GIT_BRANCH).weight(10));
        entries.push(Entry::new(Command::NewTag, "New tag…", icons::TAG));
        entries.push(Entry::new(Command::AddRemote, "Add remote…", icons::CLOUD));
        entries.push(Entry::new(
            Command::AddSubmodule,
            "Add submodule…",
            icons::PACKAGE,
        ));
        if self
            .current
            .submodules
            .as_ref()
            .is_some_and(|s| s.needing_attention() > 0)
        {
            entries.push(
                Entry::new(
                    Command::UpdateSubmodules,
                    "Update submodules",
                    icons::PACKAGE,
                )
                .detail("some are out of date")
                .weight(30),
            );
        }
        entries.push(
            Entry::new(
                Command::SearchMessages,
                "Search history…",
                icons::MAGNIFYING_GLASS,
            )
            .shortcut_opt(self.shortcut(A::Search)),
        );
        if !self.current.center.is_history() {
            entries.push(Entry::new(
                Command::ShowHistory,
                "Back to history",
                icons::ARROW_LEFT,
            ));
        }
        if head.is_some_and(|h| h.pending != crate::git::PendingOp::None) {
            entries.push(
                Entry::new(
                    Command::AbortOperation,
                    "Abort the operation in progress",
                    icons::X,
                )
                .weight(40),
            );
        }

        entries.push(
            Entry::new(
                Command::ToggleIgnored,
                "Toggle showing ignored files",
                icons::EYE,
            )
            .detail(if self.settings.show_ignored {
                "on"
            } else {
                "off"
            }),
        );
        entries.push(
            Entry::new(
                Command::ToggleSyntax,
                "Toggle syntax highlighting",
                icons::CODE,
            )
            .detail(if self.settings.syntax_highlighting {
                "on"
            } else {
                "off"
            }),
        );
        entries.push(
            Entry::new(
                Command::ToggleAutoRefresh,
                "Toggle watching for changes",
                icons::EYE,
            )
            .detail(if self.settings.auto_refresh {
                "on"
            } else {
                "off"
            }),
        );

        // Branches, stashes, and the current commit are all reachable by name.
        if let Some(refs) = &self.current.repo.refs {
            for branch in &refs.local {
                if branch.is_head {
                    continue;
                }
                entries.push(
                    Entry::new(
                        Command::Checkout(branch.name.clone()),
                        format!("Switch to {}", branch.name),
                        icons::GIT_BRANCH,
                    )
                    .detail(branch.name.clone())
                    .weight(5),
                );
                entries.push(Entry::new(
                    Command::MergeBranch(branch.name.clone()),
                    format!("Merge {} into current", branch.name),
                    icons::GIT_MERGE,
                ));
                entries.push(Entry::new(
                    Command::RebaseOnto(branch.name.clone()),
                    format!("Rebase current onto {}", branch.name),
                    icons::STACK,
                ));
            }
            for remote in &refs.remotes {
                for branch in &remote.branches {
                    let full = format!("{}/{}", remote.name, branch.name);
                    entries.push(Entry::new(
                        Command::CheckoutRemote(full.clone()),
                        format!("Check out {full}"),
                        icons::CLOUD,
                    ));
                }
            }
            for stash in &refs.stashes {
                entries.push(Entry::new(
                    Command::ApplyStash(stash.index),
                    format!("Apply stash: {}", stash.message),
                    icons::ARCHIVE,
                ));
            }
        }

        // A hash typed into the palette becomes a jump, without needing a mode.
        let query = self.palette_query.trim();
        if query.len() >= 4 && query.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Some(row) = self.current.repo.graph.as_ref().and_then(|g| {
                g.rows
                    .iter()
                    .find(|r| r.commit.id.to_string().starts_with(query))
            }) {
                entries.push(
                    Entry::new(
                        Command::GoToCommit(row.commit.id.to_string()),
                        format!("Go to {}", row.commit.short_id),
                        icons::GIT_COMMIT,
                    )
                    .detail(row.commit.summary.clone())
                    .weight(100),
                );
            }
        }

        entries
    }

    fn run_command(&mut self, ctx: &egui::Context, command: crate::ui::command::Command) {
        use crate::ui::command::Command as C;
        use crate::ui::dialog::Dialog;

        match command {
            C::OpenRepository => self.pick_folder(),
            C::SwitchTab(position) => self.activate(position),
            C::CloseTab => self.close_tab(self.current_slot),
            C::CloneRepository => self.dialog = Some(self.new_clone_dialog()),
            C::Refresh => self.refresh_all(),
            C::ToggleTheme => {
                let next = self.settings.theme.toggled();
                self.set_theme(ctx, next);
            }
            C::ToggleIgnored => {
                self.settings.show_ignored = !self.settings.show_ignored;
                self.settings.save();
                self.refresh_all();
            }
            C::ToggleSyntax => {
                self.settings.syntax_highlighting = !self.settings.syntax_highlighting;
                self.settings.save();
                self.current.repo.commit_diff = None;
                self.current.repo.staged = None;
                self.current.repo.unstaged = None;
            }
            C::ToggleAutoRefresh => {
                self.settings.auto_refresh = !self.settings.auto_refresh;
                self.settings.save();
                if self.settings.auto_refresh {
                    self.restart_watchers(ctx);
                } else {
                    self.stop_watchers();
                }
            }

            C::StageAll => self.mutate(Mutation::StageAll),
            C::UnstageAll => {
                let paths: Vec<String> = self
                    .current
                    .repo
                    .status
                    .as_ref()
                    .map(|s| s.staged().map(|e| e.path.clone()).collect())
                    .unwrap_or_default();
                if !paths.is_empty() {
                    self.mutate(Mutation::UnstageFiles(paths));
                }
            }
            C::FocusCommitMessage => {
                self.select(Selection::Workdir);
                self.current.workdir_view = WorkdirView::Unstaged;
            }
            C::DraftMessage => {
                self.select(Selection::Workdir);
                self.draft_message();
            }
            C::AmendLast => {
                self.select(Selection::Workdir);
                self.set_amending(true);
            }

            C::Fetch => self.mutate(Mutation::Fetch {
                remote: None,
                prune: true,
            }),
            C::Pull => self.mutate(Mutation::Pull(crate::git::remote::PullMode::Merge)),
            C::PullRebase => self.mutate(Mutation::Pull(crate::git::remote::PullMode::Rebase)),
            C::Push => self.open_push_dialog(None),

            C::NewBranch => {
                self.dialog = Some(Dialog::CreateBranch {
                    name: String::new(),
                    start_point: None,
                    start_label: self
                        .current
                        .repo
                        .head
                        .as_ref()
                        .map(|h| h.display_name())
                        .unwrap_or_else(|| "HEAD".to_owned()),
                    checkout: true,
                });
            }
            C::NewTag => {
                if let Some(oid) = self.current.repo.selected_commit().or_else(|| {
                    self.current
                        .repo
                        .head
                        .as_ref()
                        .and_then(|h| h.oid.as_ref())
                        .and_then(|s| git2::Oid::from_str(s).ok())
                }) {
                    self.dialog = Some(Dialog::CreateTag {
                        name: String::new(),
                        message: String::new(),
                        target: oid,
                        target_label: crate::git::repo::short_id(oid),
                    });
                }
            }
            C::StashChanges => {
                self.dialog = Some(Dialog::StashSave {
                    message: String::new(),
                    include_untracked: true,
                })
            }
            C::AddSubmodule => {
                self.dialog = Some(Dialog::AddSubmodule {
                    url: String::new(),
                    path: String::new(),
                })
            }
            C::UpdateSubmodules => self.mutate(Mutation::UpdateSubmodule(None)),
            C::AddRemote => {
                self.dialog = Some(Dialog::AddRemote {
                    name: if self.remote_names().is_empty() {
                        "origin".to_owned()
                    } else {
                        String::new()
                    },
                    url: String::new(),
                })
            }

            C::Checkout(name) => self.mutate(Mutation::Checkout(name)),
            C::CheckoutRemote(name) => self.mutate(Mutation::CheckoutRemote(name)),
            C::MergeBranch(name) => self.mutate(Mutation::Merge(name)),
            C::RebaseOnto(name) => self.mutate(Mutation::RebaseOnto(name)),
            C::ApplyStash(index) => self.mutate(Mutation::StashApply(index)),

            C::GoToCommit(hash) => {
                if let Ok(oid) = git2::Oid::from_str(&hash) {
                    self.reveal_commit(oid);
                }
            }
            C::SearchMessages => self.search_focus = true,
            C::ShowHistory => self.leave_center_view(),
            C::AbortOperation => {
                let is_rebase = self
                    .current
                    .repo
                    .head
                    .as_ref()
                    .map(|h| h.pending == crate::git::PendingOp::Rebase)
                    .unwrap_or(false);
                self.mutate(if is_rebase {
                    Mutation::RebaseAbort
                } else {
                    Mutation::AbortOperation
                });
            }
        }
    }

    /// Re-establish filesystem watchers after the setting was turned back on.
    ///
    /// Every tab, not just the visible one: a background tab with no watcher
    /// would show a badge frozen at whatever it was when watching stopped.
    fn restart_watchers(&mut self, ctx: &egui::Context) {
        // The real git directory is remembered from when each repository was
        // opened. It is not always `<worktree>/.git` — a linked worktree or a
        // submodule keeps it somewhere else entirely.
        let targets: Vec<(RepoKey, PathBuf)> = std::iter::once(&self.current)
            .chain(self.background.iter())
            .filter_map(|s| Some((s.repo.key.clone()?, s.git_dir.clone()?)))
            .collect();

        for (key, git_dir) in targets {
            match RepoWatcher::new(&key.0, &git_dir, ctx.clone()) {
                Ok(watcher) => {
                    if let Some(session) = self.session_for(&key) {
                        session.watcher = Some(watcher);
                    }
                }
                Err(e) => self.toast(Toast::error(e.user_message())),
            }
        }
    }

    /// Stop watching everything.
    fn stop_watchers(&mut self) {
        self.current.watcher = None;
        for session in &mut self.background {
            session.watcher = None;
        }
    }

    fn draw_dialog(&mut self, ctx: &egui::Context) {
        use crate::ui::dialog::DialogAction;
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let remotes = self.remote_names();
        let action = crate::ui::dialog::show(ctx, &self.palette, &mut dialog, &remotes);

        match action {
            None => self.dialog = Some(dialog),
            Some(DialogAction::Cancel) => {}
            Some(DialogAction::BrowseCloneParent) => {
                // Keep the dialog open behind the picker; the chosen folder is
                // applied when the picker answers.
                self.dialog = Some(dialog);
                self.pick_clone_parent();
            }
            Some(DialogAction::Confirm(confirmed)) => self.confirm_dialog(confirmed),
        }
    }

    fn confirm_dialog(&mut self, dialog: crate::ui::dialog::Dialog) {
        use crate::ui::dialog::Dialog;
        match dialog {
            Dialog::CreateBranch {
                name,
                start_point,
                checkout,
                ..
            } => self.mutate(Mutation::CreateBranch {
                name: name.trim().to_owned(),
                start_point,
                checkout,
            }),
            Dialog::RenameBranch { old, name } => self.mutate(Mutation::RenameBranch {
                old,
                new: name.trim().to_owned(),
            }),
            Dialog::DeleteBranch { name, force, .. } => {
                self.pending_delete = Some(name.clone());
                self.mutate(Mutation::DeleteBranch { name, force });
            }
            Dialog::DeleteTag { name } => self.mutate(Mutation::DeleteTag(name)),
            Dialog::CreateTag {
                name,
                message,
                target,
                ..
            } => self.mutate(Mutation::CreateTag {
                name: name.trim().to_owned(),
                target,
                message: (!message.trim().is_empty()).then_some(message),
            }),
            Dialog::StashSave {
                message,
                include_untracked,
            } => self.mutate(Mutation::StashSave {
                message: (!message.trim().is_empty()).then_some(message),
                include_untracked,
            }),
            Dialog::Push {
                remote,
                branch,
                set_upstream,
                force,
            } => self.mutate(Mutation::Push {
                remote,
                branch,
                set_upstream,
                mode: if force {
                    crate::git::remote::PushMode::ForceWithLease
                } else {
                    crate::git::remote::PushMode::Normal
                },
            }),
            Dialog::AddRemote { name, url } => self.mutate(Mutation::AddRemote {
                name: name.trim().to_owned(),
                url: url.trim().to_owned(),
            }),
            Dialog::AddSubmodule { url, path } => self.mutate(Mutation::AddSubmodule {
                url: url.trim().to_owned(),
                path: path.trim().to_owned(),
            }),
            Dialog::Clone { url, parent, name } => self.start_clone(url, parent, name),
            Dialog::DiscardConfirm { paths, untracked } => {
                self.mutate(if untracked {
                    Mutation::DeleteUntracked(paths)
                } else {
                    Mutation::DiscardFiles(paths)
                });
            }
            Dialog::Reset { target, kind, .. } => {
                self.mutate(Mutation::Reset { oid: target, kind })
            }
            Dialog::RebasePlan { plan, .. } => self.mutate(Mutation::RebaseInteractive(plan)),
            Dialog::Mainline {
                oid,
                chosen,
                picking,
                ..
            } => self.mutate(if picking {
                Mutation::CherryPick {
                    oid,
                    mainline: chosen,
                }
            } else {
                Mutation::Revert {
                    oid,
                    mainline: chosen,
                }
            }),
        }
    }

    /// Handle a choice from a commit's context menu.
    fn handle_commit_action(
        &mut self,
        ctx: &egui::Context,
        action: crate::ui::graph::CommitAction,
    ) {
        use crate::ui::dialog::Dialog;
        use crate::ui::graph::CommitAction as A;

        match action {
            A::CheckoutCommit(oid) => self.mutate(Mutation::CheckoutCommit(oid)),
            A::CherryPick(oid) => self.apply_commit(oid, true),
            A::Revert(oid) => self.apply_commit(oid, false),
            A::MergeInto(name) => self.mutate(Mutation::Merge(name)),
            A::RebaseOnto(name) => self.mutate(Mutation::RebaseOnto(name)),

            A::BranchFrom(oid) => {
                let label = crate::git::repo::short_id(oid);
                self.dialog = Some(Dialog::CreateBranch {
                    name: String::new(),
                    start_point: Some(oid.to_string()),
                    start_label: label,
                    checkout: true,
                });
            }
            A::TagAt(oid) => {
                self.dialog = Some(Dialog::CreateTag {
                    name: String::new(),
                    message: String::new(),
                    target: oid,
                    target_label: crate::git::repo::short_id(oid),
                });
            }
            A::Reset(oid) => {
                let dirty = self
                    .current
                    .repo
                    .status
                    .as_ref()
                    .map(|s| s.entries.len())
                    .unwrap_or(0);
                self.dialog = Some(Dialog::Reset {
                    target: oid,
                    target_label: crate::git::repo::short_id(oid),
                    kind: crate::git::merge::ResetKind::Mixed,
                    dirty,
                });
            }
            A::RebaseFrom(oid) => self.open_rebase_planner(oid),

            A::CopyHash(oid) => {
                ctx.copy_text(oid.to_string());
                self.toast(Toast::info("Copied the full hash"));
            }
            A::CopySummary(summary) => {
                ctx.copy_text(summary);
                self.toast(Toast::info("Copied the summary"));
            }
        }
    }

    /// Cherry-pick or revert a commit, asking for a mainline first if it is a
    /// merge.
    ///
    /// The parents come from the loaded graph rather than a job: the UI thread
    /// must not open the repository, and a dialog that appears a frame late in
    /// response to a menu click feels broken.
    fn apply_commit(&mut self, oid: git2::Oid, picking: bool) {
        use crate::ui::dialog::Dialog;
        let Some(commit) = self.commit_summary(oid).cloned() else {
            self.toast(Toast::error("That commit isn't loaded"));
            return;
        };

        if commit.parents.len() <= 1 {
            self.mutate(if picking {
                Mutation::CherryPick { oid, mainline: 0 }
            } else {
                Mutation::Revert { oid, mainline: 0 }
            });
            return;
        }

        let parents = commit
            .parents
            .iter()
            .enumerate()
            .map(|(index, parent)| {
                (
                    index as u32 + 1,
                    crate::git::repo::short_id(*parent),
                    self.commit_summary(*parent)
                        .map(|c| c.summary.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();

        self.dialog = Some(Dialog::Mainline {
            oid,
            short_id: commit.short_id.clone(),
            summary: commit.summary.clone(),
            parents,
            // The first parent is what git defaults to and what people almost
            // always want; pre-selecting it makes the common case one click.
            chosen: 1,
            picking,
        });
    }

    /// Build a rebase plan for everything after `base` and open the planner.
    ///
    /// The plan is built on the UI thread from the already-loaded graph rather
    /// than dispatched as a job: it is a walk over data already in memory, and
    /// a modal that appears a frame late feels broken.
    fn open_rebase_planner(&mut self, base: git2::Oid) {
        use crate::git::rebase::{RebasePlan, RebaseStep, StepAction};
        use crate::ui::dialog::Dialog;
        let Some(graph) = self.current.repo.graph.clone() else {
            return;
        };

        // Walk the first-parent chain from HEAD down to the base.
        let Some(head_oid) = self
            .current
            .repo
            .head
            .as_ref()
            .and_then(|h| h.oid.as_ref())
            .and_then(|s| git2::Oid::from_str(s).ok())
        else {
            self.toast(Toast::error("There's no HEAD to rebase"));
            return;
        };

        let mut steps = Vec::new();
        let mut cursor = Some(head_oid);
        while let Some(oid) = cursor {
            if oid == base {
                break;
            }
            let Some(row) = graph.rows.iter().find(|r| r.commit.id == oid) else {
                self.toast(Toast::error(
                    "That commit isn't in the loaded history — scroll further back first",
                ));
                return;
            };
            if row.commit.is_merge() {
                self.toast(Toast::error(
                    "That range contains a merge, which an interactive rebase can't replay",
                ));
                return;
            }
            steps.push(RebaseStep {
                oid,
                short_id: row.commit.short_id.clone(),
                summary: row.commit.summary.clone(),
                action: StepAction::Pick,
                message: String::new(),
            });
            cursor = row.commit.parents.first().copied();
        }

        if cursor.is_none() {
            self.toast(Toast::error("That commit isn't an ancestor of HEAD"));
            return;
        }
        if steps.is_empty() {
            self.toast(Toast::info("There's nothing after that commit to rebase"));
            return;
        }

        // git's todo list runs oldest first.
        steps.reverse();
        let original: Vec<git2::Oid> = steps.iter().map(|s| s.oid).collect();
        self.dialog = Some(Dialog::RebasePlan {
            plan: Box::new(RebasePlan { steps, base }),
            original,
        });
    }
}

impl eframe::App for GitupApp {
    /// Runs before every frame, and also while the window is hidden. All state
    /// transitions live here so that drawing stays a pure function of state.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.settings.save();
    }
}

// ============================================================== drawing

impl GitupApp {
    fn draw_toolbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::top(egui::Id::new("toolbar"))
            .exact_size(metrics::TOOLBAR)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(p.bg_surface)
                    .inner_margin(Margin::symmetric(space::LG as i8, 0))
                    .stroke(Stroke::NONE),
            )
            .show(ui, |ui| {
                // A single hairline under the toolbar reads cleaner than a
                // full border box.
                let bottom = ui.max_rect().bottom();
                ui.painter()
                    .hline(ui.max_rect().x_range(), bottom, Stroke::new(1.0, p.border));

                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::MD;

                    ui.label(text::icon_sized(icons::GIT_COMMIT, 17.0).color(p.accent));

                    if self.current.repo.is_open() {
                        // The full path lives here rather than in the status
                        // bar: it matters when two checkouts share a name, and
                        // is noise the rest of the time.
                        ui.label(text::strong(self.current.repo.name()).color(p.text))
                            .on_hover_text(
                                self.current
                                    .repo
                                    .key
                                    .as_ref()
                                    .map(|k| k.0.display().to_string())
                                    .unwrap_or_default(),
                            );
                        self.draw_branch_chip(ui);
                        self.draw_pending_chip(ui);
                    } else {
                        ui.label(text::strong("Gitup").color(p.text));
                    }

                    if self.current.repo.is_open() {
                        ui.add_space(space::MD);
                        self.draw_search_field(ui);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let theme_icon = match self.settings.theme {
                            ThemeMode::Dark => icons::SUN,
                            ThemeMode::Light => icons::MOON,
                        };
                        if self.icon_button(ui, icons::GEAR, "Settings").clicked() {
                            self.open_settings();
                        }

                        if self
                            .icon_button(ui, theme_icon, &self.hint("Toggle theme", A::ToggleTheme))
                            .clicked()
                        {
                            let next = self.settings.theme.toggled();
                            self.set_theme(ui.ctx(), next);
                        }

                        // The watcher keeps everything current, so a manual
                        // refresh only earns a place in the toolbar when there
                        // is no watcher to do it.
                        if self.current.repo.is_open()
                            && self.current.watcher.is_none()
                            && self
                                .icon_button(
                                    ui,
                                    icons::ARROW_CLOCKWISE,
                                    &self.hint("Refresh", A::Refresh),
                                )
                                .clicked()
                        {
                            self.refresh_all();
                        }

                        if self
                            .icon_button(
                                ui,
                                icons::FOLDER_OPEN,
                                &self.hint("Open repository", A::OpenRepository),
                            )
                            .clicked()
                        {
                            self.pick_folder();
                        }

                        if self.current.repo.is_open() {
                            ui.add_space(space::SM);
                            self.draw_remote_buttons(ui);
                            ui.add_space(space::SM);
                            self.draw_create_menu(ui);
                        }
                    });
                });
            });
    }

    fn draw_branch_chip(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(head) = &self.current.repo.head else {
            return;
        };

        let detached = matches!(head.kind, HeadKind::Detached);
        let fg = if detached {
            p.warning
        } else {
            p.text_secondary
        };

        Frame::NONE
            .fill(p.bg_raised)
            .corner_radius(CornerRadius::same(radius::PILL))
            .inner_margin(Margin::symmetric(space::MD as i8, space::XS as i8))
            .stroke(Stroke::new(1.0, p.border))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                ui.label(text::icon_sized(icons::GIT_BRANCH, 13.0).color(fg));
                ui.label(text::label(head.display_name()).color(p.text));

                if let Some(up) = &head.upstream {
                    if up.ahead > 0 {
                        ui.label(
                            text::caption(format!("{}{}", icons::ARROW_UP, up.ahead))
                                .color(p.added),
                        );
                    }
                    if up.behind > 0 {
                        ui.label(
                            text::caption(format!("{}{}", icons::ARROW_DOWN, up.behind))
                                .color(p.info),
                        );
                    }
                    if up.ahead == 0 && up.behind == 0 {
                        ui.label(text::caption("in sync").color(p.text_muted));
                    }
                } else if !detached && !head.is_empty {
                    ui.label(text::caption("no upstream").color(p.text_muted));
                }
            });
    }

    fn draw_pending_chip(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(label) = self
            .current
            .repo
            .head
            .as_ref()
            .and_then(|h| h.pending.label())
        else {
            return;
        };
        Frame::NONE
            .fill(p.warning.gamma_multiply(0.18))
            .corner_radius(CornerRadius::same(radius::PILL))
            .inner_margin(Margin::symmetric(space::MD as i8, space::XS as i8))
            .stroke(Stroke::new(1.0, p.warning.gamma_multiply(0.5)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                ui.label(text::icon_sized(icons::WARNING, 13.0).color(p.warning));
                ui.label(text::label(label).color(p.warning));
            });
    }

    /// The search field. Lives in the toolbar because searching history is a
    /// navigation action, not a mode.
    fn draw_search_field(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        // Leave room for the buttons on the right rather than letting the field
        // swallow the toolbar.
        let width = (ui.available_width() - 230.0).clamp(120.0, 340.0);

        // One control, not two. Choosing what to search and typing what to
        // search for are halves of the same question, and drawing them as two
        // separate rounded pills sitting next to each other made the toolbar
        // look like a form rather than a search box.
        Frame::NONE
            .fill(p.bg_raised)
            .corner_radius(CornerRadius::same(radius::MD))
            .inner_margin(Margin::symmetric(space::SM as i8, 0))
            .show(ui, |ui| {
                ui.set_height(26.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::XS;
                    ui.label(text::icon_sized(icons::MAGNIFYING_GLASS, 12.0).color(p.text_muted));

                    let field = ui.add_sized(
                        Vec2::new(width, 22.0),
                        egui::TextEdit::singleline(&mut self.current.search_query)
                            .hint_text(self.current.search_kind.hint())
                            // The container already draws the frame; a second
                            // one inside it is the box-in-a-box look.
                            .frame(Frame::NONE),
                    );
                    if self.search_focus {
                        field.request_focus();
                        self.search_focus = false;
                    }
                    // Enter runs the search; typing alone does not, because each
                    // search spawns a git process and doing that per keystroke
                    // is wasteful.
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.run_search();
                    }

                    if !self.current.search_query.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    text::icon_sized(icons::X, 11.0).color(p.text_muted),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::new(18.0, 18.0)),
                            )
                            .on_hover_text("Clear")
                            .clicked()
                    {
                        self.current.search_query.clear();
                        self.leave_center_view();
                    }

                    ui.painter().vline(
                        ui.cursor().left(),
                        egui::Rangef::new(ui.max_rect().top() + 5.0, ui.max_rect().bottom() - 5.0),
                        Stroke::new(1.0, p.border),
                    );
                    ui.add_space(space::XS);

                    // Flattened so the selector reads as part of the field
                    // rather than as a button parked against it.
                    let kind = self.current.search_kind;
                    let widgets = &mut ui.style_mut().visuals.widgets;
                    for state in [
                        &mut widgets.inactive,
                        &mut widgets.hovered,
                        &mut widgets.active,
                    ] {
                        state.weak_bg_fill = Color32::TRANSPARENT;
                        state.bg_fill = Color32::TRANSPARENT;
                        state.bg_stroke = Stroke::NONE;
                    }
                    egui::ComboBox::from_id_salt("search_kind")
                        .selected_text(text::caption(kind.label()).color(p.text_secondary))
                        .width(76.0)
                        .show_ui(ui, |ui| {
                            for candidate in crate::git::search::SearchKind::all() {
                                ui.selectable_value(
                                    &mut self.current.search_kind,
                                    candidate,
                                    candidate.label(),
                                )
                                .on_hover_text(candidate.hint());
                            }
                        });
                });
            });
    }

    /// Why pulling isn't possible right now, if it isn't.
    ///
    /// One place decides, so the button, its tooltip, and the palette can never
    /// disagree — and so a disabled control always has a reason to give. A
    /// greyed-out button with no explanation is indistinguishable from a broken
    /// one, which is exactly how it gets reported.
    fn pull_blocker(&self) -> Option<String> {
        if self.current_is_busy() {
            return Some("Wait for the current operation to finish".to_owned());
        }
        let Some(head) = self.current.repo.head.as_ref() else {
            return Some("No repository is open".to_owned());
        };
        if head.is_empty {
            return Some("This repository has no commits yet".to_owned());
        }
        let Some(branch) = head.branch_name() else {
            return Some("HEAD is detached — check out a branch to pull".to_owned());
        };
        if self.remote_names().is_empty() {
            return Some("This repository has no remotes — add one first".to_owned());
        }
        if !head.can_pull() {
            return Some(format!(
                "‘{branch}’ isn't tracking a remote branch. Push it with \u{2018}Track this \
                 branch on the remote\u{2019} to set one up."
            ));
        }
        None
    }

    /// Why pushing isn't possible right now, if it isn't.
    fn push_blocker(&self) -> Option<String> {
        if self.current_is_busy() {
            return Some("Wait for the current operation to finish".to_owned());
        }
        let Some(head) = self.current.repo.head.as_ref() else {
            return Some("No repository is open".to_owned());
        };
        if head.is_empty {
            return Some("There are no commits to push".to_owned());
        }
        if head.branch_name().is_none() {
            return Some("HEAD is detached — check out a branch to push".to_owned());
        }
        if self.remote_names().is_empty() {
            return Some("This repository has no remotes — add one first".to_owned());
        }
        None
    }

    /// Why fetching isn't possible right now, if it isn't.
    fn fetch_blocker(&self) -> Option<String> {
        if self.current_is_busy() {
            return Some("Wait for the current operation to finish".to_owned());
        }
        if self.remote_names().is_empty() {
            return Some("This repository has no remotes — add one first".to_owned());
        }
        None
    }

    /// Fetch, pull, and push, with the counts that make them meaningful.
    fn draw_remote_buttons(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let upstream = self
            .current
            .repo
            .head
            .as_ref()
            .and_then(|h| h.upstream.clone());
        let (ahead, behind) = upstream
            .as_ref()
            .map(|u| (u.ahead, u.behind))
            .unwrap_or((0, 0));

        // Right-to-left layout, so these read Fetch · Pull · Push on screen.
        let push = self.action_button(
            ui,
            icons::CLOUD_ARROW_UP,
            (ahead > 0).then(|| ahead.to_string()),
            if ahead > 0 { p.added } else { p.text_secondary },
            &self.hint("Push", A::Push),
            self.push_blocker(),
        );
        if push.clicked() {
            self.open_push_dialog(None);
        }

        let pull = self.action_button(
            ui,
            icons::CLOUD_ARROW_DOWN,
            (behind > 0).then(|| behind.to_string()),
            if behind > 0 { p.info } else { p.text_secondary },
            &self.hint("Pull", A::Pull),
            self.pull_blocker(),
        );
        if pull.clicked() {
            self.mutate(Mutation::Pull(crate::git::remote::PullMode::Merge));
        }

        let fetch = self.action_button(
            ui,
            icons::ARROWS_CLOCKWISE,
            None,
            p.text_secondary,
            &self.hint("Fetch all remotes", A::Fetch),
            self.fetch_blocker(),
        );
        if fetch.clicked() {
            self.mutate(Mutation::Fetch {
                remote: None,
                prune: true,
            });
        }
    }

    fn action_button(
        &self,
        ui: &mut egui::Ui,
        glyph: &str,
        badge: Option<String>,
        colour: Color32,
        tip: &str,
        // `Some` when the action is unavailable, carrying the reason to show.
        blocker: Option<String>,
    ) -> egui::Response {
        let p = self.palette;
        let enabled = blocker.is_none();
        let label = match &badge {
            Some(count) => format!("{glyph} {count}"),
            None => glyph.to_owned(),
        };
        let mut rich = egui::RichText::new(label)
            .font(egui::FontId::new(13.0, text::icon_family()))
            .color(if enabled { colour } else { p.text_muted });
        if badge.is_some() {
            // The count is proportional text sitting beside an icon glyph; the
            // icon family falls back to Inter for it.
            rich = rich.size(12.0);
        }
        let button = egui::Button::new(rich)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(30.0, 26.0));
        let response = ui.add_enabled(enabled, button);
        // A disabled widget never shows `on_hover_text`, so the reason has to
        // go through the disabled variant or it is shown to nobody.
        match blocker {
            Some(reason) => response.on_disabled_hover_text(reason),
            None => response.on_hover_text(tip),
        }
    }

    /// The `+` menu: everything that creates something.
    fn draw_create_menu(&mut self, ui: &mut egui::Ui) {
        use crate::ui::dialog::Dialog;
        let p = self.palette;
        let response = ui
            .add(
                egui::Button::new(text::icon_sized(icons::PLUS, 15.0).color(p.text_secondary))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(28.0, 26.0)),
            )
            .on_hover_text("Create…");

        let mut requested: Option<Dialog> = None;
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(190.0);
            if ui.button("New branch…").clicked() {
                requested = Some(Dialog::CreateBranch {
                    name: String::new(),
                    start_point: None,
                    start_label: self
                        .current
                        .repo
                        .head
                        .as_ref()
                        .map(|h| h.display_name())
                        .unwrap_or_else(|| "HEAD".to_owned()),
                    checkout: true,
                });
                ui.close();
            }
            if ui.button("New tag…").clicked() {
                if let Some(oid) = self.current.repo.selected_commit().or_else(|| {
                    self.current
                        .repo
                        .head
                        .as_ref()
                        .and_then(|h| h.oid.as_ref())
                        .and_then(|s| git2::Oid::from_str(s).ok())
                }) {
                    requested = Some(Dialog::CreateTag {
                        name: String::new(),
                        message: String::new(),
                        target: oid,
                        target_label: crate::git::repo::short_id(oid),
                    });
                }
                ui.close();
            }
            if ui.button("Stash changes…").clicked() {
                requested = Some(Dialog::StashSave {
                    message: String::new(),
                    include_untracked: true,
                });
                ui.close();
            }
            ui.separator();
            if ui.button("Add submodule…").clicked() {
                requested = Some(Dialog::AddSubmodule {
                    url: String::new(),
                    path: String::new(),
                });
                ui.close();
            }
            if ui.button("Add remote…").clicked() {
                requested = Some(Dialog::AddRemote {
                    name: if self.remote_names().is_empty() {
                        "origin".to_owned()
                    } else {
                        String::new()
                    },
                    url: String::new(),
                });
                ui.close();
            }
            if ui.button("Clone repository…").clicked() {
                requested = Some(self.new_clone_dialog());
                ui.close();
            }
        });

        if let Some(dialog) = requested {
            self.dialog = Some(dialog);
        }
    }

    fn new_clone_dialog(&self) -> crate::ui::dialog::Dialog {
        crate::ui::dialog::Dialog::Clone {
            url: String::new(),
            parent: self
                .current
                .repo
                .key
                .as_ref()
                .and_then(|k| k.0.parent().map(std::path::Path::to_path_buf))
                .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
            name: String::new(),
        }
    }

    fn icon_button(&self, ui: &mut egui::Ui, glyph: &str, tip: &str) -> egui::Response {
        let p = self.palette;
        ui.add(
            egui::Button::new(text::icon_sized(glyph, 15.0).color(p.text_secondary))
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::NONE)
                .min_size(Vec2::new(28.0, 26.0)),
        )
        .on_hover_text(tip)
    }

    fn draw_tab_bar(&mut self, ui: &mut egui::Ui) {
        // With nothing open there is nothing to switch between, and the
        // welcome screen already offers a way in.
        if !self.current.is_open() {
            return;
        }
        let p = self.palette;

        let order = self.tab_order();
        let tabs: Vec<crate::ui::tabs::TabInfo<'_>> = order
            .iter()
            .map(|tab| {
                let session = self.session_at(*tab);
                let busy = session
                    .repo
                    .key
                    .as_ref()
                    .is_some_and(|key| self.repo_is_busy(key));
                crate::ui::tabs::TabInfo { session, busy }
            })
            .collect();
        let active = order
            .iter()
            .position(|tab| *tab == TabRef::Current)
            .unwrap_or(0);

        let response = egui::Panel::top(egui::Id::new("tab_bar"))
            .exact_size(crate::ui::tabs::height())
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg_surface))
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    Stroke::new(1.0, p.border),
                );
                let bar = crate::ui::tabs::TabBar {
                    palette: &p,
                    tabs,
                    active,
                };
                bar.show(ui)
            })
            .inner;

        if let Some(position) = response.activate {
            self.activate(position);
        }
        if let Some(position) = response.close {
            self.close_tab(position);
        }
        if response.open_new {
            self.pick_folder_for_new_tab();
        }
    }

    /// A strip naming the operation in progress, with the two things you can
    /// do about it. Without this, a conflicted merge looks like a broken
    /// repository rather than a task waiting to be finished.
    fn draw_operation_banner(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(pending) = self.current.repo.head.as_ref().map(|h| h.pending) else {
            return;
        };
        let Some(label) = pending.label() else { return };

        let conflicts = self
            .current
            .repo
            .status
            .as_ref()
            .map(|s| s.conflict_count)
            .unwrap_or(0);
        let is_rebase = pending == crate::git::PendingOp::Rebase;

        egui::Panel::top(egui::Id::new("operation_banner"))
            .exact_size(34.0)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(p.tinted(p.warning, 0.16))
                    .inner_margin(Margin::symmetric(space::LG as i8, 0)),
            )
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().bottom(),
                    Stroke::new(1.0, p.warning.gamma_multiply(0.4)),
                );
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    ui.label(text::icon_sized(icons::WARNING, 14.0).color(p.warning));
                    ui.label(text::medium(label).color(p.text));
                    ui.label(
                        text::caption(if conflicts > 0 {
                            format!(
                                "{} to resolve",
                                crate::util::words::plural(conflicts, "conflict")
                            )
                        } else if is_rebase {
                            "ready to continue".to_owned()
                        } else {
                            "ready to commit".to_owned()
                        })
                        .color(p.text_secondary),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let abort = egui::Button::new(text::label("Abort").color(p.danger))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::new(1.0, p.danger.gamma_multiply(0.6)))
                            .corner_radius(CornerRadius::same(radius::SM))
                            .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
                        if ui
                            .add(abort)
                            .on_hover_text("Put everything back the way it was")
                            .clicked()
                        {
                            self.mutate(if is_rebase {
                                Mutation::RebaseAbort
                            } else {
                                Mutation::AbortOperation
                            });
                        }

                        if is_rebase {
                            let can_continue = conflicts == 0;
                            let button =
                                egui::Button::new(text::label("Continue").color(if can_continue {
                                    p.text_on_accent
                                } else {
                                    p.text_muted
                                }))
                                .fill(if can_continue { p.accent } else { p.bg_raised })
                                .stroke(Stroke::NONE)
                                .corner_radius(CornerRadius::same(radius::SM))
                                .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
                            if ui.add_enabled(can_continue, button).clicked() {
                                self.mutate(Mutation::RebaseContinue);
                            }
                            if ui
                                .add(
                                    egui::Button::new(text::label("Skip").color(p.text_secondary))
                                        .fill(p.bg_raised)
                                        .stroke(Stroke::NONE)
                                        .corner_radius(CornerRadius::same(radius::SM))
                                        .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT)),
                                )
                                .on_hover_text("Leave this commit out and carry on")
                                .clicked()
                            {
                                self.mutate(Mutation::RebaseSkip);
                            }
                        } else if conflicts > 0
                            && ui
                                .add(
                                    egui::Button::new(
                                        text::label("Resolve").color(p.text_on_accent),
                                    )
                                    .fill(p.danger)
                                    .stroke(Stroke::NONE)
                                    .corner_radius(CornerRadius::same(radius::SM))
                                    .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT)),
                                )
                                .clicked()
                        {
                            self.select(Selection::Workdir);
                            self.current.workdir_view = WorkdirView::Conflicts;
                        }
                    });
                });
            });
    }

    fn draw_statusbar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::bottom(egui::Id::new("statusbar"))
            .exact_size(metrics::STATUSBAR)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(p.bg_surface)
                    .inner_margin(Margin::symmetric(space::LG as i8, 0)),
            )
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    Stroke::new(1.0, p.border),
                );
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::MD;

                    if let Some(progress) = self.visible_progress() {
                        ui.label(text::icon_sized(icons::CIRCLE_NOTCH, 12.0).color(p.accent));
                        ui.label(text::caption(&progress.label).color(p.text_secondary));
                        if let Some(fraction) = progress.fraction() {
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .desired_width(140.0)
                                    .desired_height(6.0)
                                    .corner_radius(CornerRadius::same(radius::PILL))
                                    .fill(p.accent),
                            );
                            ui.label(
                                text::caption(format!(
                                    "{}/{}",
                                    progress.done,
                                    progress.total.unwrap_or(0)
                                ))
                                .color(p.text_muted),
                            );
                        }
                        return;
                    }

                    let labels = self.jobs.active_labels();
                    if labels.is_empty() {
                        let (glyph, color, tip) = if self.current.watcher.is_some() {
                            (icons::EYE, p.text_muted, "Watching for changes")
                        } else if self.current.repo.is_open() {
                            (
                                icons::EYE_SLASH,
                                p.warning,
                                "Not watching — refresh manually",
                            )
                        } else {
                            // Nothing is open, so there is nothing to report.
                            // "Idle" described the application to itself.
                            return;
                        };
                        ui.label(text::icon_sized(glyph, 12.0).color(color));
                        ui.label(text::caption(tip).color(p.text_muted));
                    } else {
                        ui.label(text::icon_sized(icons::CIRCLE_NOTCH, 12.0).color(p.accent));
                        ui.label(text::caption(labels.join(" · ")).color(p.text_secondary));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(head) = &self.current.repo.head {
                            if let Some(up) = &head.upstream {
                                ui.label(text::caption(&up.name).color(p.text_muted));
                                ui.label(
                                    text::icon_sized(icons::CLOUD_ARROW_UP, 12.0)
                                        .color(p.text_muted),
                                );
                            }
                        }
                    });
                });
            });
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::Panel::left(egui::Id::new("sidebar"))
            .resizable(true)
            .default_size(self.settings.sidebar_width)
            .size_range(metrics::SIDEBAR_MIN..=480.0)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                ui.painter().vline(
                    ui.max_rect().right(),
                    ui.max_rect().y_range(),
                    Stroke::new(1.0, p.border),
                );
                self.remember_layout(|s| &mut s.sidebar_width, ui.available_width());

                let status = self.current.repo.status.clone();
                let sidebar = crate::ui::sidebar::Sidebar {
                    palette: &p,
                    refs: self.current.repo.refs.as_deref(),
                    submodules: self.current.submodules.as_deref(),
                    selection: self.current.repo.selection,
                    uncommitted: status.as_ref().map(|s| s.entries.len()),
                    conflicts: status.as_ref().map(|s| s.conflict_count).unwrap_or(0),
                };
                let response = sidebar.show(ui);
                if let Some(action) = response.action {
                    self.handle_sidebar(ui.ctx(), action);
                }
            });
    }

    fn handle_sidebar(&mut self, ctx: &egui::Context, action: crate::ui::sidebar::SidebarAction) {
        use crate::ui::dialog::Dialog;
        use crate::ui::sidebar::SidebarAction as A;

        match action {
            A::Select(selection) => self.select(selection),
            A::Reveal(oid) => self.reveal_commit(oid),
            A::CheckoutBranch(name) => self.mutate(Mutation::Checkout(name)),
            A::CheckoutRemote(name) => self.mutate(Mutation::CheckoutRemote(name)),
            A::NewBranchFrom { start, label } => {
                self.dialog = Some(Dialog::CreateBranch {
                    name: String::new(),
                    start_point: Some(start),
                    start_label: label,
                    checkout: true,
                });
            }
            A::RenameBranch(old) => {
                self.dialog = Some(Dialog::RenameBranch {
                    name: old.clone(),
                    old,
                });
            }
            A::DeleteBranch(name) => {
                self.dialog = Some(Dialog::DeleteBranch {
                    name,
                    force: false,
                    warning: None,
                });
            }
            A::PushBranch(branch) => self.open_push_dialog(Some(branch)),
            A::ClearUpstream(branch) => self.mutate(Mutation::SetUpstream {
                branch,
                upstream: None,
            }),
            A::DeleteTag(name) => self.dialog = Some(Dialog::DeleteTag { name }),
            A::UpdateSubmodule(path) => self.mutate(Mutation::UpdateSubmodule(path)),
            A::OpenSubmodule(path) => {
                // Opening a submodule means opening it as a repository in its
                // own right — the same as any other repository, because that is
                // exactly what it is.
                if let Some(key) = self.current.repo.key.clone() {
                    let full = key.0.join(&path);
                    if crate::git::submodule::is_repository(&full) {
                        // In its own tab: you almost always want the parent
                        // still there to come back to.
                        self.open_in_new_tab(full);
                    } else {
                        self.toast(Toast::error(format!("{path} hasn't been initialized yet")));
                    }
                }
            }
            A::RemoveSubmodule(path) => self.mutate(Mutation::RemoveSubmodule(path)),
            A::AddSubmodule => {
                self.dialog = Some(Dialog::AddSubmodule {
                    url: String::new(),
                    path: String::new(),
                })
            }
            A::StashApply(index) => self.mutate(Mutation::StashApply(index)),
            A::StashPop(index) => self.mutate(Mutation::StashPop(index)),
            A::StashDrop(index) => self.mutate(Mutation::StashDrop(index)),
            A::CopyText(text) => {
                ctx.copy_text(text.clone());
                self.toast(Toast::info(format!("Copied ‘{text}’")));
            }
        }
    }

    /// Queue a repository change.
    fn mutate(&mut self, action: Mutation) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        self.jobs.dispatch(Job::Mutate { repo: key, action });
    }

    fn open_push_dialog(&mut self, branch: Option<String>) {
        use crate::ui::dialog::Dialog;
        let branch = branch
            .or_else(|| match self.current.repo.head.as_ref().map(|h| &h.kind) {
                Some(HeadKind::Branch(name)) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if branch.is_empty() {
            self.toast(Toast::error("Detached HEAD — check out a branch to push"));
            return;
        }

        let remotes = self.remote_names();
        let Some(remote) = remotes.first().cloned() else {
            self.toast(Toast::error("This repository has no remotes"));
            return;
        };
        // Offer to set upstream only when there isn't one; suggesting it
        // otherwise implies something is wrong.
        let set_upstream = self
            .current
            .repo
            .refs
            .as_ref()
            .and_then(|r| r.branch(&branch))
            .map(|b| b.upstream.is_none())
            .unwrap_or(true);

        self.dialog = Some(Dialog::Push {
            remote,
            branch,
            set_upstream,
            force: false,
        });
    }

    fn remote_names(&self) -> Vec<String> {
        self.current
            .repo
            .refs
            .as_ref()
            .map(|r| r.remotes.iter().map(|g| g.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Open the blame view for a path. Exposed for tests, which have no way to
    /// click the button that normally does this.
    pub fn open_blame_for_test(&mut self, path: &str) {
        self.open_blame(path.to_owned(), None);
    }

    /// Find a commit's metadata wherever it happens to be loaded.
    ///
    /// Search results and file history can name commits outside the graph
    /// window, so looking only in the graph would leave the detail pane blank
    /// for exactly the commits the user just went looking for.
    fn commit_summary(&self, oid: git2::Oid) -> Option<&crate::git::CommitSummary> {
        self.current
            .repo
            .graph
            .as_ref()
            .and_then(|g| g.rows.iter().find(|r| r.commit.id == oid))
            .map(|r| &r.commit)
            .or_else(|| {
                self.current
                    .search_results
                    .as_ref()
                    .and_then(|r| r.commits.iter().find(|c| c.id == oid))
            })
    }

    /// Blame a file, optionally as of a specific commit.
    fn open_blame(&mut self, path: String, at: Option<git2::Oid>) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        self.current.blame = None;
        self.current.center = CenterView::Blame {
            path: path.clone(),
            at,
        };
        self.jobs.dispatch(Job::Blame {
            repo: key,
            path,
            at,
            theme: self.highlight_theme(),
        });
    }

    /// Re-blame the current file as it was *before* a commit.
    ///
    /// This is how you walk back through the history of a line: each step
    /// answers "and what was there before this change?".
    fn blame_before(&mut self, oid: git2::Oid) {
        let CenterView::Blame { path, .. } = self.current.center.clone() else {
            return;
        };
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        // The parent has to be resolved before dispatching, and the UI thread
        // must not open the repository to do it — so ask the graph, which
        // already knows the parents of every loaded commit.
        let parent = self
            .commit_summary(oid)
            .and_then(|c| c.parents.first().copied());

        match parent {
            Some(parent) => self.open_blame(path, Some(parent)),
            None => {
                self.toast(Toast::info(
                    "That's the first commit — nothing came before it",
                ));
                let _ = key;
            }
        }
    }

    fn open_file_history(&mut self, path: String) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        self.current.search_results = None;
        self.current.center = CenterView::FileHistory { path: path.clone() };
        self.jobs.dispatch(Job::FileHistory {
            repo: key,
            path,
            limit: 500,
        });
    }

    fn run_search(&mut self) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        if self.current.search_query.trim().is_empty() {
            self.leave_center_view();
            return;
        }
        self.current.search_results = None;
        self.current.center = CenterView::Search;
        self.jobs.dispatch(Job::Search {
            repo: key,
            kind: self.current.search_kind,
            query: self.current.search_query.clone(),
            limit: 500,
        });
    }

    /// Select a commit and scroll it into view, if it is in the loaded graph.
    fn reveal_commit(&mut self, oid: git2::Oid) {
        let known = self
            .current
            .repo
            .graph
            .as_ref()
            .is_some_and(|g| g.rows.iter().any(|r| r.commit.id == oid));
        if known {
            self.select(Selection::Commit(oid));
            self.current.pending_scroll = Some(oid);
        } else {
            // The commit exists but hasn't been walked yet — reachable only
            // from a ref outside the loaded window.
            self.toast(Toast::info("That commit is outside the loaded history"));
            self.grow_graph();
        }
    }

    fn draw_center(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                if let Some(label) = self.current.center.label() {
                    self.draw_breadcrumb(ui, &label);
                }

                // History on top, detail below. The split is resizable because
                // reading a long diff and scanning a long history want opposite
                // proportions — and it is remembered, because a resize that is
                // forgotten on every launch is not really adjustable at all.
                //
                // The default is a share of the centre, not a pixel height. The
                // detail pane pays for two header bands and the commit box out
                // of its own share rather than the window's, so a height that
                // looks generous leaves the diff itself with very little.
                let centre = ui.available_height();
                let furniture = self.detail_furniture();
                egui::Panel::bottom(egui::Id::new("detail_pane"))
                    .resizable(true)
                    .default_size(crate::ui::layout::detail_height(
                        centre,
                        furniture,
                        self.settings.detail_share,
                    ))
                    .size_range(90.0..=2000.0)
                    .show_separator_line(false)
                    .frame(Frame::NONE.fill(p.bg_base))
                    .show(ui, |ui| {
                        ui.painter().hline(
                            ui.max_rect().x_range(),
                            ui.max_rect().top(),
                            Stroke::new(1.0, p.border),
                        );
                        let height = ui.available_height();
                        self.remember_share(crate::ui::layout::share_of(centre, furniture, height));
                        self.draw_detail(ui);
                    });

                match self.current.center.clone() {
                    CenterView::History => self.draw_history(ui),
                    CenterView::Search | CenterView::FileHistory { .. } => {
                        self.draw_commit_list(ui)
                    }
                    CenterView::Blame { .. } => self.draw_blame(ui),
                }
            });
    }

    /// A bar naming the current view, with a way back to the history.
    fn draw_breadcrumb(&mut self, ui: &mut egui::Ui, label: &str) {
        let p = self.palette;
        Frame::NONE
            .fill(p.bg_surface)
            .inner_margin(Margin::symmetric(space::LG as i8, space::SM as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    if ui
                        .add(
                            egui::Button::new(
                                text::icon_sized(icons::ARROW_LEFT, 13.0).color(p.text_secondary),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .min_size(Vec2::new(22.0, 20.0)),
                        )
                        .on_hover_text("Back to history  ⎋")
                        .clicked()
                    {
                        self.leave_center_view();
                    }
                    ui.label(text::label(label).color(p.text_secondary));

                    if let Some(results) = &self.current.search_results {
                        if !self.current.center.is_history()
                            && !matches!(self.current.center, CenterView::Blame { .. })
                        {
                            let count = results.commits.len();
                            ui.label(
                                text::caption(if results.truncated {
                                    format!("{count}+ commits")
                                } else {
                                    crate::util::words::plural(count, "commit")
                                })
                                .color(p.text_muted),
                            );
                        }
                    }
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.border),
        );
    }

    fn leave_center_view(&mut self) {
        self.current.center = CenterView::History;
        self.current.blame = None;
        self.current.search_results = None;
    }

    /// Search results and file history share this list: both are "commits that
    /// matched something", drawn without lanes because they are not contiguous.
    fn draw_commit_list(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(results) = self.current.search_results.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::caption("Searching…").color(p.text_muted));
            });
            return;
        };
        if results.commits.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::icon_sized(icons::MAGNIFYING_GLASS, 24.0).color(p.text_muted));
                ui.add_space(space::MD);
                ui.label(text::subtitle("Nothing found").color(p.text_secondary));
            });
            return;
        }

        let now = crate::util::time::now();
        let selected = self.current.repo.selected_commit();
        let mut clicked = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 26.0, results.commits.len(), |ui, range| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                for index in range {
                    let commit = &results.commits[index];
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 26.0),
                        egui::Sense::click(),
                    );
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let is_selected = selected == Some(commit.id);
                    if is_selected {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::ZERO, p.selected);
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                rect.left_top(),
                                Vec2::new(2.0, rect.height()),
                            ),
                            CornerRadius::ZERO,
                            p.accent,
                        );
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, p.hover);
                    }

                    let painter = ui.painter();
                    let cy = rect.center().y;
                    let right = rect.right() - space::LG;

                    painter.text(
                        egui::pos2(right, cy),
                        egui::Align2::RIGHT_CENTER,
                        &commit.short_id,
                        egui::FontId::new(text::size::CAPTION, text::mono_family()),
                        p.text_muted,
                    );
                    painter.text(
                        egui::pos2(right - 70.0, cy),
                        egui::Align2::RIGHT_CENTER,
                        crate::util::time::relative(commit.time, now),
                        egui::FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                        p.text_muted,
                    );
                    painter.text(
                        egui::pos2(right - 180.0, cy),
                        egui::Align2::RIGHT_CENTER,
                        &commit.author_name,
                        egui::FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
                        p.text_muted,
                    );

                    let x = rect.left() + space::LG;
                    let galley = painter.layout(
                        commit.summary.clone(),
                        egui::FontId::new(text::size::BODY, egui::FontFamily::Proportional),
                        p.text,
                        (right - 280.0 - x).max(60.0),
                    );
                    painter
                        .with_clip_rect(egui::Rect::from_min_max(
                            egui::pos2(x, rect.top()),
                            egui::pos2(right - 280.0, rect.bottom()),
                        ))
                        .galley(egui::pos2(x, cy - galley.size().y / 2.0), galley, p.text);

                    if response.clicked() {
                        clicked = Some(commit.id);
                    }
                }
            });

        if let Some(oid) = clicked {
            self.select(Selection::Commit(oid));
        }
    }

    fn draw_blame(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(result) = self.current.blame.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::caption("Computing blame…").color(p.text_muted));
            });
            return;
        };

        let view = crate::ui::blame::BlameView {
            palette: &p,
            result: &result,
            highlighted: self.current.repo.selected_commit(),
        };
        let response = view.show(ui);

        if let Some(oid) = response.selected {
            self.select(Selection::Commit(oid));
        }
        if let Some(oid) = response.reblame_before {
            self.blame_before(oid);
        }
    }

    /// The lower half: what the selected row actually changed.
    fn draw_detail(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;

        // A `Panel` persists whatever height its content reported the first
        // time it was drawn. The placeholder states below are short, so without
        // this the pane would be permanently stuck at the height of the words
        // "Computing diff…" — claim the full area regardless of what is in it.
        ui.set_min_size(ui.available_size());

        let Some(selection) = self.current.repo.selection else {
            // "Select a commit" in a repository with no commits asks for
            // something impossible, which reads as the app being stuck rather
            // than as an empty state.
            let nothing_to_select = self.current.repo.head.as_ref().is_some_and(|h| h.is_empty)
                || self
                    .current
                    .repo
                    .graph
                    .as_ref()
                    .is_some_and(|g| g.rows.is_empty());
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.35);
                ui.label(
                    text::caption(if nothing_to_select {
                        "Nothing to show yet"
                    } else {
                        "Select a commit"
                    })
                    .color(p.text_muted),
                );
            });
            return;
        };

        self.draw_detail_header(ui, selection);

        let Some(model) = self
            .current
            .repo
            .active_diff(self.current.workdir_view)
            .cloned()
        else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::caption("Computing diff…").color(p.text_muted));
            });
            return;
        };

        if selection == Selection::Workdir && self.current.workdir_view == WorkdirView::Conflicts {
            self.draw_conflicts(ui);
            return;
        }

        // The commit box lives at the bottom of the working-tree view, so the
        // message is written next to what it describes.
        if selection == Selection::Workdir {
            let p = self.palette;
            egui::Panel::bottom(egui::Id::new("commit_box"))
                .resizable(true)
                .default_size(self.settings.commit_box_height)
                .size_range(72.0..=400.0)
                .show_separator_line(false)
                .frame(Frame::NONE.fill(p.bg_surface))
                .show(ui, |ui| {
                    ui.painter().hline(
                        ui.max_rect().x_range(),
                        ui.max_rect().top(),
                        Stroke::new(1.0, p.border),
                    );
                    let height = ui.available_height();
                    self.remember_layout(|s| &mut s.commit_box_height, height);
                    self.draw_commit_box(ui);
                });
        }

        let direction = match selection {
            Selection::Workdir => Some(match self.current.workdir_view {
                WorkdirView::Staged => crate::ui::diff::StageDirection::Staged,
                // Conflicts never reach here; the view above returns first.
                _ => crate::ui::diff::StageDirection::Unstaged,
            }),
            Selection::Commit(_) => None,
        };

        let pane = crate::ui::diff::DiffPane {
            palette: &p,
            model: &model,
            active_file: self.current.repo.active_file.as_deref(),
            list_width: 250.0,
            direction,
            layout: self.settings.diff_layout,
            line_selection: &self.current.line_selection,
            focused: self.current.focus == crate::state::Focus::Files,
        };
        let response = pane.show(ui);

        if let Some(path) = response.selected_file {
            if self.current.repo.active_file.as_deref() != Some(path.as_str()) {
                self.current.line_selection.clear();
                self.current.selection_anchor = None;
            }
            self.current.repo.active_file = Some(path);
            self.current.focus = crate::state::Focus::Files;
        }
        if let Some((hunk, line, shift)) = response.line_clicked {
            self.toggle_line(hunk, line, shift);
        }
        if let Some((hunk, action)) = response.hunk_action {
            self.act_on_hunk(&model, hunk, action);
        }
        if let Some((path, action)) = response.file_action {
            self.act_on_file(&path, action);
        }
        if let Some(path) = response.blame_file {
            self.open_blame(path, self.current.repo.selected_commit());
        }

        if response.body_height > 0.0 {
            self.diff_body_height = response.body_height;
        }

        self.handle_file_keys(ui.ctx(), &model, direction);
        if let Some(path) = response.file_history {
            self.open_file_history(path);
        }
    }

    /// Extend, or toggle, the diff-line selection.
    fn toggle_line(&mut self, hunk: usize, line: usize, shift: bool) {
        if shift {
            if let Some((anchor_hunk, anchor_line)) = self.current.selection_anchor {
                if anchor_hunk == hunk {
                    // A shift-click inside one hunk selects the run between the
                    // anchor and the click, which is how every list behaves.
                    let (lo, hi) = if anchor_line <= line {
                        (anchor_line, line)
                    } else {
                        (line, anchor_line)
                    };
                    for index in lo..=hi {
                        self.current.line_selection.insert((hunk, index));
                    }
                    return;
                }
            }
        }

        if !self.current.line_selection.remove(&(hunk, line)) {
            self.current.line_selection.insert((hunk, line));
        }
        self.current.selection_anchor = Some((hunk, line));
    }

    fn act_on_hunk(
        &mut self,
        model: &std::sync::Arc<crate::git::DiffModel>,
        hunk: usize,
        action: crate::ui::diff::RowAction,
    ) {
        let Some(path) = self.current.repo.active_file.clone() else {
            return;
        };
        self.dispatch_partial(
            model.clone(),
            path,
            vec![crate::git::stage::HunkSelection::whole(hunk)],
            action,
        );
    }

    /// Apply the current line selection.
    fn act_on_selection(&mut self, action: crate::ui::diff::RowAction) {
        let Some(model) = self
            .current
            .repo
            .active_diff(self.current.workdir_view)
            .cloned()
        else {
            return;
        };
        let Some(path) = self.current.repo.active_file.clone() else {
            return;
        };
        if self.current.line_selection.is_empty() {
            return;
        }

        // Group the selected lines by hunk; the patch builder wants one
        // selection per hunk, not a flat list.
        let mut by_hunk: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for (hunk, line) in &self.current.line_selection {
            by_hunk.entry(*hunk).or_default().push(*line);
        }
        let selections = by_hunk
            .into_iter()
            .map(|(hunk_index, lines)| crate::git::stage::HunkSelection {
                hunk_index,
                lines: Some(lines),
            })
            .collect();

        self.dispatch_partial(model, path, selections, action);
    }

    fn dispatch_partial(
        &mut self,
        model: std::sync::Arc<crate::git::DiffModel>,
        path: String,
        selections: Vec<crate::git::stage::HunkSelection>,
        action: crate::ui::diff::RowAction,
    ) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        let kind = match action {
            crate::ui::diff::RowAction::Stage => PartialKind::Stage,
            crate::ui::diff::RowAction::Unstage => PartialKind::Unstage,
            crate::ui::diff::RowAction::Discard => PartialKind::Discard,
        };
        self.jobs.dispatch(Job::Mutate {
            repo: key,
            action: Mutation::Partial {
                model,
                path,
                selections,
                kind,
            },
        });
    }

    /// Arrows and Space in the changed-file list.
    ///
    /// Staging by keyboard is what makes working through a large change
    /// bearable, and there was no way to do it at all: the file list took mouse
    /// clicks only, and the arrow keys belonged to the commit graph whatever
    /// you were doing.
    fn handle_file_keys(
        &mut self,
        ctx: &egui::Context,
        model: &crate::git::diff::DiffModel,
        direction: Option<crate::ui::diff::StageDirection>,
    ) {
        if self.current.focus != crate::state::Focus::Files {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
            self.current.focus = crate::state::Focus::History;
            return;
        }
        if model.files.is_empty() {
            return;
        }

        let (delta, toggle) = ctx.input_mut(|i| {
            let mut delta = 0i64;
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                delta += 1;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                delta -= 1;
            }
            (
                delta,
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
            )
        });

        let at = |app: &Self| {
            app.current
                .repo
                .active_file
                .as_deref()
                .and_then(|active| model.files.iter().position(|f| f.path == active))
        };

        if delta != 0 {
            let last = model.files.len() - 1;
            let next = match at(self) {
                Some(index) => (index as i64 + delta).clamp(0, last as i64) as usize,
                // Nothing selected yet: enter the list from whichever end the
                // key came from.
                None if delta > 0 => 0,
                None => last,
            };
            self.current.repo.active_file = Some(model.files[next].path.clone());
            self.current.line_selection.clear();
            self.current.selection_anchor = None;
        }

        if toggle {
            let (Some(index), Some(direction)) = (at(self), direction) else {
                return;
            };
            let path = model.files[index].path.clone();

            // Move on *before* staging. The file is about to leave this list,
            // and landing back at the top after every keystroke would make
            // working down a long list impossible. Whatever is chosen here
            // survives the refresh, because `ensure_active_file` only steps in
            // when the selected file has actually gone.
            let following = model
                .files
                .get(index + 1)
                .or_else(|| index.checked_sub(1).and_then(|i| model.files.get(i)));
            self.current.repo.active_file = following.map(|f| f.path.clone());
            self.current.line_selection.clear();
            self.current.selection_anchor = None;

            let action = match direction {
                crate::ui::diff::StageDirection::Unstaged => crate::ui::diff::RowAction::Stage,
                crate::ui::diff::StageDirection::Staged => crate::ui::diff::RowAction::Unstage,
            };
            self.act_on_file(&path, action);
        }
    }

    fn act_on_file(&mut self, path: &str, action: crate::ui::diff::RowAction) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        let paths = vec![path.to_owned()];
        let mutation = match action {
            crate::ui::diff::RowAction::Stage => Mutation::StageFiles(paths),
            crate::ui::diff::RowAction::Unstage => Mutation::UnstageFiles(paths),
            crate::ui::diff::RowAction::Discard => Mutation::DiscardFiles(paths),
        };
        self.jobs.dispatch(Job::Mutate {
            repo: key,
            action: mutation,
        });
    }

    fn draw_detail_header(&mut self, ui: &mut egui::Ui, selection: Selection) {
        let p = self.palette;
        Frame::NONE
            .fill(p.bg_surface)
            .inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match selection {
                    Selection::Workdir => self.draw_workdir_header(ui),
                    Selection::Commit(oid) => self.draw_commit_header(ui, oid),
                }
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().bottom(),
            Stroke::new(1.0, p.border),
        );
    }

    fn draw_workdir_header(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let status = self.current.repo.status.clone().unwrap_or_default();
        let staged_files = self
            .current
            .repo
            .staged
            .as_ref()
            .map(|m| m.files.len())
            .unwrap_or(0);
        let unstaged_files = self
            .current
            .repo
            .unstaged
            .as_ref()
            .map(|m| m.files.len())
            .unwrap_or(0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::SM;

            // A segmented control rather than separate panes: the same file
            // often appears on both sides, and switching is how you compare
            // what you have staged against what you have not.
            let mut tabs = vec![
                (WorkdirView::Unstaged, unstaged_files),
                (WorkdirView::Staged, staged_files),
            ];
            if status.conflict_count > 0 {
                // Conflicts take precedence: nothing else can be finished until
                // they are dealt with.
                tabs.insert(0, (WorkdirView::Conflicts, status.conflict_count));
            }
            for (view, count) in tabs {
                let active = self.current.workdir_view == view;
                let label = format!("{} {count}", view.label());
                let accent = if view == WorkdirView::Conflicts {
                    p.danger
                } else {
                    p.accent
                };
                let button = egui::Button::new(text::label(label).color(if active {
                    p.text_on_accent
                } else {
                    p.text_secondary
                }))
                .fill(if active { accent } else { p.bg_raised })
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(radius::SM))
                .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
                if ui.add(button).clicked() && !active {
                    self.current.workdir_view = view;
                    self.current.line_selection.clear();
                    self.current.selection_anchor = None;
                    self.current.repo.active_file = None;
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.draw_layout_toggle(ui);
                if status.unstaged_count > 0 || status.untracked_count > 0 {
                    let stage_all =
                        egui::Button::new(text::label("Stage all").color(p.text_secondary))
                            .fill(p.bg_raised)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(radius::SM))
                            .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
                    if ui.add(stage_all).clicked() {
                        if let Some(key) = self.current.repo.key.clone() {
                            self.jobs.dispatch(Job::Mutate {
                                repo: key,
                                action: Mutation::StageAll,
                            });
                        }
                    }
                }

                if !self.current.line_selection.is_empty() {
                    self.draw_selection_actions(ui);
                }
            });
        });
    }

    /// The action bar that appears once diff lines are selected.
    fn draw_selection_actions(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let count = self.current.line_selection.len();
        let direction = self.current.workdir_view;

        let mut requested = None;
        if direction == WorkdirView::Unstaged {
            let discard = egui::Button::new(text::label("Discard").color(p.danger))
                .fill(p.danger.gamma_multiply(0.18))
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(radius::SM))
                .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
            if ui
                .add(discard)
                .on_hover_text("Throw these lines away — this cannot be undone")
                .clicked()
            {
                requested = Some(crate::ui::diff::RowAction::Discard);
            }
        }

        let primary = match direction {
            WorkdirView::Staged => crate::ui::diff::RowAction::Unstage,
            _ => crate::ui::diff::RowAction::Stage,
        };
        let label = match (primary, count) {
            (crate::ui::diff::RowAction::Stage, 1) => "Stage 1 line".to_owned(),
            (crate::ui::diff::RowAction::Stage, n) => format!("Stage {n} lines"),
            (_, 1) => "Unstage 1 line".to_owned(),
            (_, n) => format!("Unstage {n} lines"),
        };
        let button = egui::Button::new(text::label(label).color(p.text_on_accent))
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(radius::SM))
            .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));
        if ui.add(button).clicked() {
            requested = Some(primary);
        }

        if let Some(action) = requested {
            self.act_on_selection(action);
        }
    }

    fn draw_conflicts(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(conflicts) = self.current.conflicts.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::caption("Reading conflicts…").color(p.text_muted));
            });
            return;
        };

        let mut buffer = std::mem::take(&mut self.current.conflict_buffer);
        let mut view = crate::ui::conflict::ConflictView {
            palette: &p,
            conflicts: &conflicts,
            active: self.current.conflict_file.as_deref(),
            editing: self.current.conflict_editing,
            edit_buffer: &mut buffer,
        };
        let response = view.show(ui);
        self.current.conflict_buffer = buffer;

        if let Some(path) = response.selected_file {
            self.current.conflict_file = Some(path);
            self.current.conflict_buffer.clear();
        }
        if response.toggle_edit {
            self.current.conflict_editing = !self.current.conflict_editing;
            self.current.conflict_buffer.clear();
        }
        if let Some((path, resolution)) = response.resolve {
            self.current.conflict_buffer.clear();
            self.mutate(Mutation::ResolveConflict { path, resolution });
        }
        if let Some((path, content)) = response.resolve_edited {
            self.current.conflict_buffer.clear();
            self.current.conflict_editing = false;
            self.mutate(Mutation::ResolveConflictContent { path, content });
        }
    }

    /// Message editor and the commit button.
    fn draw_commit_box(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        ui.set_min_size(ui.available_size());
        let staged = self
            .current
            .repo
            .status
            .as_ref()
            .map(|s| s.staged_count)
            .unwrap_or(0);
        let conflicts = self
            .current
            .repo
            .status
            .as_ref()
            .map(|s| s.conflict_count)
            .unwrap_or(0);

        Frame::NONE
            .inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());

                let (subject, _) = crate::git::commit::split_message(&self.current.commit_message);
                let subject_len = subject.chars().count();
                let nothing_staged = self
                    .current
                    .repo
                    .staged
                    .as_ref()
                    .is_none_or(|d| d.is_empty());

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    ui.label(text::overline("Commit message").color(p.text_muted));

                    // A live subject-length hint rather than a rule: git does
                    // not enforce this, and neither should the app.
                    if subject_len > crate::git::commit::SUBJECT_SOFT_LIMIT {
                        let over = subject_len > crate::git::commit::SUBJECT_HARD_LIMIT;
                        ui.label(
                            text::caption(format!("subject {subject_len} chars")).color(if over {
                                p.warning
                            } else {
                                p.text_muted
                            }),
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_commit = (staged > 0 || self.current.amending)
                            && conflicts == 0
                            && !self.current.commit_message.trim().is_empty();

                        let label = if self.current.amending {
                            "Amend"
                        } else {
                            "Commit"
                        };
                        let button = egui::Button::new(text::medium(label).color(if can_commit {
                            p.text_on_accent
                        } else {
                            p.text_muted
                        }))
                        .fill(if can_commit { p.accent } else { p.bg_raised })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(radius::MD))
                        .min_size(Vec2::new(88.0, metrics::BUTTON_ACTION));

                        // The disabled reasons need `on_disabled_hover_text`;
                        // `on_hover_text` shows nothing once a widget is
                        // disabled, which leaves the button looking broken.
                        let response = ui.add_enabled(can_commit, button);
                        let response = if can_commit {
                            response.on_hover_text(Chord::cmd(egui::Key::Enter).display())
                        } else if conflicts > 0 {
                            response.on_disabled_hover_text("Resolve the conflicts first")
                        } else if staged == 0 {
                            response.on_disabled_hover_text("Nothing is staged")
                        } else {
                            response.on_disabled_hover_text("Write a message first")
                        };
                        if response.clicked() {
                            self.do_commit();
                        }

                        let mut amending = self.current.amending;
                        if ui
                            .checkbox(
                                &mut amending,
                                text::caption("Amend").color(p.text_secondary),
                            )
                            .on_hover_text("Replace the previous commit instead of adding one")
                            .changed()
                        {
                            self.set_amending(amending);
                        }

                        ui.add_space(space::SM);

                        // Labelled rather than a bare icon: a wand on its own
                        // says nothing about what it would do, and this is the
                        // one control here that is not already familiar from
                        // every other Git client. Styled like "Stage all", the
                        // other secondary action in this panel.
                        let can_draft = self.can_draft_message();
                        let tint = if can_draft {
                            p.text_secondary
                        } else {
                            p.text_muted
                        };
                        let draft =
                            egui::Button::new(text::icon_label(icons::MAGIC_WAND, "Draft", tint))
                                .fill(p.bg_raised)
                                .stroke(Stroke::NONE)
                                .corner_radius(CornerRadius::same(radius::SM))
                                .min_size(Vec2::new(0.0, metrics::BUTTON_COMPACT));

                        let response = ui.add_enabled(can_draft, draft);
                        let response =
                            if can_draft {
                                response.on_hover_text(self.hint(
                                    "Draft a message from the staged changes",
                                    A::DraftMessage,
                                ))
                            } else if nothing_staged {
                                response.on_disabled_hover_text(
                                    "Stage something first — a draft describes what is staged",
                                )
                            } else {
                                response.on_disabled_hover_text(
                                "Clear the message first; drafting would replace what you wrote",
                            )
                            };
                        if response.clicked() {
                            self.draft_message();
                        }
                    });
                });

                ui.add_space(space::XS);
                let response = ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.current.commit_message)
                        .hint_text("Summarize the change…")
                        .desired_rows(2)
                        .font(egui::TextStyle::Body),
                );

                // Any edit makes the text the user's rather than ours, so a
                // later draft will refuse to overwrite it.
                if response.changed() {
                    self.current.message_is_draft = false;
                }
            });
    }

    /// Whether drafting a message would destroy something the user wrote.
    ///
    /// Replacing a previous draft is safe; replacing typed text is not. This is
    /// what lets the button stay available after a first draft — stage more,
    /// draft again — without ever being the thing that loses a message.
    fn can_draft_message(&self) -> bool {
        self.current
            .repo
            .staged
            .as_ref()
            .is_some_and(|d| !d.is_empty())
            && (self.current.commit_message.trim().is_empty() || self.current.message_is_draft)
    }

    /// Put a drafted message in the box.
    fn draft_message(&mut self) {
        if !self.can_draft_message() {
            let reason = if self
                .current
                .repo
                .staged
                .as_ref()
                .is_none_or(|d| d.is_empty())
            {
                "Stage something first — a draft describes what is staged"
            } else {
                "Clear the message first; drafting would replace what you wrote"
            };
            self.toast(Toast::info(reason));
            return;
        }

        let Some(staged) = self.current.repo.staged.clone() else {
            return;
        };

        // Read the house style off the history already on screen. A repository
        // whose graph has not loaded yet simply gets the plain form, which is
        // the safer default of the two.
        let convention = match &self.current.repo.graph {
            Some(page) => crate::git::message::Convention::detect(
                page.rows.iter().map(|row| row.commit.summary.as_str()),
            ),
            None => crate::git::message::Convention::Plain,
        };

        match crate::git::message::draft(&staged, convention) {
            Some(drafted) => {
                self.current.commit_message = drafted.render();
                self.current.message_is_draft = true;
                self.select(Selection::Workdir);
            }
            None => self.toast(Toast::info("Nothing staged to describe")),
        }
    }

    // ------------------------------------------------------------- identity

    /// Show the settings sheet, re-reading anything that could have changed.
    fn open_settings(&mut self) {
        self.settings_open = true;
        // Dropped rather than kept: the identity belongs to whichever tab is
        // showing, and the config may have been edited elsewhere since.
        self.identity = None;
    }

    /// Ask for the identity behind the tab being shown.
    fn load_identity(&mut self) {
        self.jobs.dispatch(Job::ReadIdentity {
            repo: self.current.repo.key.clone(),
        });
    }

    /// Point the editable fields at whichever level is selected.
    fn reseed_identity_draft(&mut self) {
        self.identity_draft = self
            .identity
            .as_ref()
            .map(|i| i.at(self.identity_scope).clone())
            .unwrap_or_default();
    }

    /// Whether the fields differ from what is stored, and so are worth saving.
    fn identity_is_dirty(&self) -> bool {
        let stored = self
            .identity
            .as_ref()
            .map(|i| i.at(self.identity_scope).clone())
            .unwrap_or_default();
        stored.name.trim() != self.identity_draft.name.trim()
            || stored.email.trim() != self.identity_draft.email.trim()
    }

    /// Write the edited fields to the selected config.
    fn save_identity(&mut self) {
        // The repository level needs a repository; the global level does not.
        let repo = self.current.repo.key.clone();
        if self.identity_scope == crate::git::identity::Scope::Repository && repo.is_none() {
            self.toast(Toast::info("Open a repository first"));
            return;
        }
        self.jobs.dispatch(Job::SetIdentity {
            repo,
            scope: self.identity_scope,
            identity: self.identity_draft.clone(),
        });
    }

    fn set_amending(&mut self, amending: bool) {
        self.current.amending = amending;
        if amending && self.current.commit_message.trim().is_empty() {
            // Pre-fill with the message being replaced; amending usually means
            // adjusting it, not rewriting from nothing. This must come from
            // HEAD, not from the newest row in the graph — another branch's tip
            // can easily be more recent than the commit being amended.
            if let Some(head) = &self.current.repo.head {
                if !head.summary.is_empty() {
                    self.current.commit_message = head.summary.clone();
                    // The previous commit's own words, not something drafted;
                    // nothing should silently replace them.
                    self.current.message_is_draft = false;
                }
            }
        }
    }

    fn do_commit(&mut self) {
        let Some(key) = self.current.repo.key.clone() else {
            return;
        };
        let staged = self
            .current
            .repo
            .status
            .as_ref()
            .map(|s| s.staged_count)
            .unwrap_or(0);
        if staged == 0 && !self.current.amending {
            self.toast(Toast::info("Nothing is staged"));
            return;
        }
        let message = self.current.commit_message.clone();
        if message.trim().is_empty() {
            // Fired from a key binding with an empty box: put the user where
            // they can type rather than only complaining.
            self.select(Selection::Workdir);
            self.toast(Toast::error("A commit needs a message"));
            return;
        }
        let mode = if self.current.amending {
            crate::git::commit::CommitMode::Amend
        } else {
            crate::git::commit::CommitMode::Normal
        };
        self.jobs.dispatch(Job::Mutate {
            repo: key,
            action: Mutation::Commit { message, mode },
        });
    }

    /// The unified / side-by-side switch, shown wherever a diff is.
    fn draw_layout_toggle(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let current = self.settings.diff_layout;
        let glyph = match current {
            crate::ui::diff::DiffLayout::Unified => icons::ROWS,
            crate::ui::diff::DiffLayout::SideBySide => icons::COLUMNS,
        };
        if ui
            .add(
                egui::Button::new(text::icon_sized(glyph, 13.0).color(p.text_muted))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(24.0, 20.0)),
            )
            .on_hover_text(format!("{} — click to switch", current.label()))
            .clicked()
        {
            self.settings.diff_layout = current.toggled();
            self.settings.save();
        }
    }

    fn draw_commit_header(&self, ui: &mut egui::Ui, oid: git2::Oid) {
        let p = self.palette;
        let Some(c) = self.commit_summary(oid) else {
            ui.label(text::caption("Loading…").color(p.text_muted));
            return;
        };

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::SM;
            ui.label(text::medium(&c.summary).color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let full = c.id.to_string();
                if ui
                    .add(
                        egui::Button::new(text::hash(&c.short_id).color(p.text_muted))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                    )
                    .on_hover_text(format!("{full}\n\nClick to copy"))
                    .clicked()
                {
                    ui.ctx().copy_text(full);
                }
            });
        });
        ui.add_space(space::XS);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::SM;
            ui.label(text::caption(&c.author_name).color(p.text_secondary));
            ui.label(text::caption("·").color(p.text_muted));
            ui.label(
                text::caption(crate::util::time::date_time(c.time, c.tz_offset_minutes))
                    .color(p.text_muted),
            );
            if c.is_merge() {
                ui.label(text::caption("·").color(p.text_muted));
                ui.label(
                    text::caption("merge — showing changes against the first parent")
                        .color(p.warning),
                );
            }
        });
    }

    fn draw_history(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        let Some(page) = self.current.repo.graph.clone() else {
            self.draw_history_skeleton(ui);
            return;
        };

        if page.rows.is_empty() && !self.current.repo.has_uncommitted() {
            let head_empty = self.current.repo.head.as_ref().is_some_and(|h| h.is_empty);
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.label(text::icon_sized(icons::GIT_COMMIT, 28.0).color(p.text_muted));
                ui.add_space(space::MD);
                ui.label(
                    text::subtitle(if head_empty {
                        "No commits yet"
                    } else {
                        "No history to show"
                    })
                    .color(p.text_secondary),
                );
                if head_empty {
                    ui.add_space(space::XS);
                    ui.label(
                        text::caption("Stage some files and make the first commit.")
                            .color(p.text_muted),
                    );
                }
            });
            return;
        }

        let show_workdir = self.current.repo.has_uncommitted();
        let workdir_count = self
            .current
            .repo
            .status
            .as_ref()
            .map(|s| s.entries.len())
            .unwrap_or(0);

        // Resolve a pending reveal to a row index; the view scrolls it into
        // the middle rather than just to the edge, so there is context around it.
        let scroll_to = self.current.pending_scroll.take().and_then(|oid| {
            page.rows
                .iter()
                .position(|r| r.commit.id == oid)
                .map(|i| i + usize::from(show_workdir))
        });

        let view = crate::ui::graph::GraphView {
            palette: &p,
            page: &page,
            selection: self.current.repo.selection,
            show_workdir_row: show_workdir,
            workdir_count,
            has_more: page.has_more,
            scroll_to,
        };
        let response = view.show(ui);

        if let Some(selection) = response.selected {
            self.current.focus = crate::state::Focus::History;
            self.select(selection);
        }
        if response.wants_more {
            self.grow_graph();
        }
        if let Some(action) = response.action {
            self.handle_commit_action(ui.ctx(), action);
        }

        // Arrow keys move through history only when history is what the user
        // is working in. Driving the graph unconditionally meant that pressing
        // Down while picking through changed files threw you back into the
        // commit list and lost your place.
        if self.current.focus != crate::state::Focus::History {
            return;
        }
        // Right steps into the changed-file list, left comes back. Without a
        // way in, the file list could only be reached with the mouse — which
        // makes keyboard staging useless to anyone who did not start there.
        if self.current.repo.selection == Some(Selection::Workdir)
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
        {
            self.current.focus = crate::state::Focus::Files;
            return;
        }
        let delta = ui.ctx().input_mut(|i| {
            let mut d = 0i64;
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                d += 1;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                d -= 1;
            }
            d
        });
        if delta != 0 {
            if let Some(next) = crate::ui::graph::step_selection(
                &page,
                show_workdir,
                self.current.repo.selection,
                delta,
            ) {
                self.select(next);
            }
        }
    }

    fn draw_history_skeleton(&self, ui: &mut egui::Ui) {
        let p = self.palette;
        let rows = (ui.available_height() / crate::ui::graph::row_height()) as usize;
        for i in 0..rows.min(24) {
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), crate::ui::graph::row_height()),
                egui::Sense::hover(),
            );
            let dot = egui::pos2(rect.left() + 15.0, rect.center().y);
            ui.painter().circle_filled(dot, 3.0, p.bg_raised);
            let w = rect.width() * (0.18 + 0.10 * ((i % 4) as f32));
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 34.0, rect.center().y - 4.0),
                    Vec2::new(w, 8.0),
                ),
                CornerRadius::same(radius::SM),
                p.bg_raised,
            );
        }
    }

    fn draw_welcome(&mut self, ui: &mut egui::Ui) {
        let p = self.palette;
        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(p.bg_base))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.16);
                    ui.set_max_width(440.0);

                    ui.label(text::icon_sized(icons::GIT_COMMIT, 40.0).color(p.accent));
                    ui.add_space(space::MD);
                    ui.label(text::display("Gitup").color(p.text));
                    ui.add_space(space::XS);
                    ui.label(
                        text::body("Open a repository, or drop one onto this window.")
                            .color(p.text_secondary),
                    );
                    ui.add_space(space::XL);

                    // Cloning was reachable only from the command palette,
                    // which you have to already know about — so someone
                    // holding a URL and opening the app for the first time had
                    // nowhere to go.
                    ui.horizontal(|ui| {
                        let width = 190.0;
                        let spacing = space::MD;
                        // Centre the pair rather than the first of them.
                        let inset = (ui.available_width() - (width * 2.0 + spacing)).max(0.0) / 2.0;
                        ui.add_space(inset);
                        ui.spacing_mut().item_spacing.x = spacing;

                        let open = egui::Button::new(
                            text::medium("Open Repository…").color(p.text_on_accent),
                        )
                        .fill(p.accent)
                        .corner_radius(CornerRadius::same(radius::MD))
                        .min_size(Vec2::new(width, metrics::BUTTON_HERO));
                        if ui.add(open).clicked() {
                            self.pick_folder();
                        }

                        let clone =
                            egui::Button::new(text::medium("Clone…").color(p.text_secondary))
                                .fill(p.bg_raised)
                                .stroke(Stroke::new(1.0, p.border_strong))
                                .corner_radius(CornerRadius::same(radius::MD))
                                .min_size(Vec2::new(width, metrics::BUTTON_HERO));
                        if ui.add(clone).clicked() {
                            self.dialog = Some(self.new_clone_dialog());
                        }
                    });

                    let recent = self.settings.existing_recent();
                    if !recent.is_empty() {
                        ui.add_space(space::XXL);
                        ui.horizontal(|ui| {
                            ui.label(text::overline("Recent").color(p.text_muted));
                        });
                        ui.add_space(space::SM);
                        for path in recent {
                            if self.draw_recent_row(ui, &path) {
                                self.open_repo(path);
                                break;
                            }
                        }
                    }

                    ui.add_space(space::XXL);
                    ui.label(
                        text::caption(format!(
                            "Press {} for the command palette",
                            self.shortcut(A::CommandPalette).unwrap_or_default()
                        ))
                        .color(p.text_muted),
                    );
                });
            });
    }

    fn draw_recent_row(&self, ui: &mut egui::Ui, path: &std::path::Path) -> bool {
        let p = self.palette;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius::MD), p.hover);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let painter = ui.painter();
        let cy = rect.center().y;
        painter.text(
            egui::pos2(rect.left() + space::MD, cy),
            egui::Align2::LEFT_CENTER,
            icons::FOLDER_OPEN,
            text::icon_font(14.0),
            p.text_muted,
        );
        let galley = painter.layout_no_wrap(
            name,
            egui::FontId::new(text::size::BODY, egui::FontFamily::Proportional),
            p.text,
        );
        let name_w = galley.size().x;
        painter.galley(
            egui::pos2(rect.left() + 30.0, cy - galley.size().y / 2.0),
            galley,
            p.text,
        );
        painter.text(
            egui::pos2(rect.left() + 30.0 + name_w + space::MD, cy),
            egui::Align2::LEFT_CENTER,
            shorten_path(path),
            egui::FontId::new(text::size::CAPTION, egui::FontFamily::Proportional),
            p.text_muted,
        );

        response.clicked()
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let p = self.palette;
        let mut dismissed = None;

        egui::Area::new("toasts".into())
            .anchor(
                egui::Align2::RIGHT_BOTTOM,
                Vec2::new(-space::XL, -(metrics::STATUSBAR + space::XL)),
            )
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (i, toast) in self.toasts.iter().enumerate() {
                    let (accent, glyph) = match toast.kind {
                        ToastKind::Error => (p.danger, icons::WARNING_CIRCLE),
                        ToastKind::Success => (p.added, icons::CHECK_CIRCLE),
                        ToastKind::Info => (p.info, icons::INFO),
                    };
                    Frame::NONE
                        .fill(p.bg_overlay)
                        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(radius::MD))
                        .inner_margin(Margin::symmetric(space::LG as i8, space::MD as i8))
                        .shadow(ui.style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            // A fixed width so the message has something to
                            // wrap against: git's explanations run to a
                            // sentence or two, and a horizontal layout would
                            // otherwise clip them at the edge of the card.
                            ui.set_width(340.0);
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = space::MD;
                                // Right-to-left first, so the close button
                                // claims its space before the text is laid out.
                                ui.with_layout(Layout::right_to_left(egui::Align::TOP), |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                text::icon_sized(icons::X, 12.0)
                                                    .color(p.text_muted),
                                            )
                                            .fill(Color32::TRANSPARENT)
                                            .stroke(Stroke::NONE),
                                        )
                                        .clicked()
                                    {
                                        dismissed = Some(i);
                                    }

                                    ui.with_layout(Layout::left_to_right(egui::Align::TOP), |ui| {
                                        ui.label(text::icon_sized(glyph, 15.0).color(accent));
                                        ui.add(
                                            egui::Label::new(text::body(&toast.text).color(p.text))
                                                .wrap(),
                                        );
                                    });
                                });
                            });
                        });
                    ui.add_space(space::SM);
                }
            });

        if let Some(i) = dismissed {
            self.toasts.remove(i);
        }
    }
}

/// Show a path relative to home where possible; absolute paths are long and
/// the interesting part is the tail.
fn shorten_path(path: &std::path::Path) -> String {
    let full = path.display().to_string();
    if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    full
}
