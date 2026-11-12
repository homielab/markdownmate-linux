//! Tabs: one window shows several notes as `adw::TabView` pages. The window has one editor
//! view and one preview; each tab has its own text buffer, so its undo history, cursor and
//! selection stay with it. Switching saves the note shown and puts the next tab's buffer in
//! the view; a tab restored from the last session reads its note the first time it is shown.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use crate::editor::Editor;
use crate::state::{Mode, OpenFilesIn, TabState};

/// What a tab that is not shown keeps.
pub(crate) struct TabData {
    pub(crate) path: Option<PathBuf>,
    pub(crate) mode: Mode,
    /// The note's buffer as the tab was left, and its text as saved then; `None` until the
    /// tab is first shown.
    pub(crate) text: Option<(sourceview5::Buffer, String)>,
}

fn title_of(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

fn label(page: &adw::TabPage, path: Option<&Path>) {
    page.set_title(&title_of(path));
    page.set_tooltip(&path.map(|p| p.display().to_string()).unwrap_or_default());
}

/// The tab context menu (right-click).
pub(crate) fn tab_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("Move to New Window"), Some("win.move-tab-to-window"));
    menu.append(Some("Close Other Tabs"), Some("win.close-other-tabs"));
    menu
}

impl Editor {
    fn add_page(&self, position: Option<i32>) -> adw::TabPage {
        let child = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let page = match position {
            Some(i) => self.tab_view.insert(&child, i),
            None => {
                let parent = self.tab_view.selected_page();
                self.tab_view.add_page(&child, parent.as_ref())
            }
        };
        label(&page, None);
        page
    }

    /// Makes the window's first page the open note's; wires the pages.
    pub(crate) fn init_tabs(self: &Rc<Self>) {
        let page = self.add_page(Some(0));
        self.tab_current.replace(Some(page));
        let weak = Rc::downgrade(self);
        self.tab_view.connect_selected_page_notify(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.tab_selected();
            }
        });
        let weak = Rc::downgrade(self);
        self.tab_view.connect_close_page(move |_, page| {
            if let Some(editor) = weak.upgrade() {
                editor.tab_closing(page);
            }
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(self);
        self.tab_view.connect_page_reordered(move |_, _, _| {
            if let Some(editor) = weak.upgrade() {
                editor.remember();
            }
        });
        let weak = Rc::downgrade(self);
        self.tab_view.connect_setup_menu(move |_, page| {
            if let (Some(editor), Some(page)) = (weak.upgrade(), page) {
                editor.tab_menu_page.replace(Some(page.clone()));
            }
        });
    }

    /// Keeps the selected tab's title and tooltip on its note.
    pub(crate) fn sync_tab(&self) {
        if let Some(page) = self.tab_current.borrow().as_ref() {
            label(page, self.path.borrow().as_deref());
        }
    }

    /// Every tab's note and mode in order, and the selected one's index.
    pub(crate) fn tab_states(&self) -> (Vec<TabState>, usize) {
        let current = self.tab_current.borrow().clone();
        let data = self.tab_data.borrow();
        let mut tabs = Vec::new();
        let mut selected = 0;
        for i in 0..self.tab_view.n_pages() {
            let page = self.tab_view.nth_page(i);
            if Some(&page) == current.as_ref() {
                selected = tabs.len();
                tabs.push(TabState {
                    note: self.path.borrow().clone(),
                    mode: self.mode.get(),
                });
            } else if let Some((_, d)) = data.iter().find(|(p, _)| *p == page) {
                tabs.push(TabState {
                    note: d.path.clone(),
                    mode: d.mode,
                });
            }
        }
        if tabs.is_empty() {
            tabs.push(TabState {
                note: self.path.borrow().clone(),
                mode: self.mode.get(),
            });
        }
        (tabs, selected)
    }

    /// The other tabs of a restored window (the selected one is already open), in order.
    pub(crate) fn restore_tabs(&self, tabs: &[TabState], selected: usize) {
        if tabs.len() < 2 || self.tab_view.n_pages() != 1 {
            return;
        }
        for (i, tab) in tabs.iter().enumerate() {
            if i == selected {
                continue;
            }
            let page = self.add_page(Some(i32::try_from(i).unwrap_or(i32::MAX)));
            label(&page, tab.note.as_deref());
            self.tab_data.borrow_mut().push((
                page,
                TabData {
                    path: tab.note.clone(),
                    mode: tab.mode,
                    text: None,
                },
            ));
        }
    }

    /// The selected page changed: save and park the note shown, then show the new page's.
    fn tab_selected(self: &Rc<Self>) {
        if self.tab_busy.get() {
            return;
        }
        let Some(new) = self.tab_view.selected_page() else {
            return;
        };
        let prev = self.tab_current.borrow().clone();
        if prev.as_ref() == Some(&new) {
            return;
        }
        // No page shown: the one that was has just closed (already saved).
        let closed = prev.is_none();
        if let Some(prev) = prev {
            if !self.save() {
                // Stay on the note that could not be saved (a toast says why).
                self.tab_busy.set(true);
                self.tab_view.set_selected_page(&prev);
                self.tab_busy.set(false);
                return;
            }
            self.keep_position();
            let data = TabData {
                path: self.path.borrow().clone(),
                mode: self.mode.get(),
                text: Some((self.buffer(), self.saved.borrow().clone())),
            };
            self.tab_data.borrow_mut().push((prev, data));
        }
        self.tab_current.replace(Some(new.clone()));
        let data = {
            let mut all = self.tab_data.borrow_mut();
            all.iter()
                .position(|(p, _)| *p == new)
                .map(|i| all.remove(i).1)
        };
        let data = data.unwrap_or(TabData {
            path: None,
            mode: Mode::Edit,
            text: None,
        });
        self.tab_busy.set(true);
        self.show_tab(data, closed);
        self.tab_busy.set(false);
    }

    /// Puts a tab in the editor. `closed`: the buffer shown until now belongs to no tab.
    fn show_tab(self: &Rc<Self>, data: TabData, closed: bool) {
        let old = self.buffer();
        // The note shown until now is saved; nothing of it may follow into the next buffer
        // (an auto-save would write that buffer over it).
        self.leave_note();
        match (data.path, data.text) {
            (Some(path), Some((buffer, saved))) => {
                self.set_buffer(&buffer);
                self.show_loaded(&path, saved);
            }
            (path, _) => {
                self.set_buffer(&self.make_buffer());
                if let Some(path) = &path {
                    self.open(path);
                }
                // An empty tab, or a note gone or unreadable meanwhile.
                if self.path.borrow().is_none() {
                    self.close_note();
                }
            }
        }
        if closed && let Some(spelling) = self.spelling.get() {
            spelling.forget(&old);
        }
        if self.path.borrow().is_some() && self.mode.get() != data.mode {
            self.set_mode(data.mode);
        }
        self.sync_tab();
    }

    fn has_page(&self, page: &adw::TabPage) -> bool {
        (0..self.tab_view.n_pages()).any(|i| self.tab_view.nth_page(i) == *page)
    }

    /// Forgets a tab that is not shown (its page is closing).
    fn drop_tab(&self, page: &adw::TabPage) {
        let data = {
            let mut all = self.tab_data.borrow_mut();
            all.iter()
                .position(|(p, _)| p == page)
                .map(|i| all.remove(i).1)
        };
        if let (Some((buffer, _)), Some(spelling)) =
            (data.and_then(|d| d.text), self.spelling.get())
        {
            spelling.forget(&buffer);
        }
    }

    /// `close-page`: the X button, Ctrl+W, or closing the others.
    fn tab_closing(self: &Rc<Self>, page: &adw::TabPage) {
        if self.tab_view.n_pages() <= 1 {
            self.tab_view.close_page_finish(page, false);
            self.window.close();
            return;
        }
        let is_current = self.tab_current.borrow().as_ref() == Some(page);
        if is_current {
            if !self.save() {
                self.tab_view.close_page_finish(page, false);
                return;
            }
            self.keep_position();
            self.tab_current.replace(None);
            self.tab_view.close_page_finish(page, true);
        } else {
            self.drop_tab(page);
            self.tab_view.close_page_finish(page, true);
            // Selecting another tab saves the session; closing one that is not shown does not.
            self.remember();
        }
    }

    /// A new empty tab, selected. False (and no tab) when the note shown cannot be saved, so
    /// the window stays on it.
    pub(crate) fn new_tab(self: &Rc<Self>) -> bool {
        let page = self.add_page(None);
        self.tab_data.borrow_mut().push((
            page.clone(),
            TabData {
                path: None,
                mode: self.mode.get(),
                text: None,
            },
        ));
        self.tab_view.set_selected_page(&page);
        if self.tab_current.borrow().as_ref() == Some(&page) {
            return true;
        }
        self.tab_view.close_page(&page);
        false
    }

    pub(crate) fn close_tab(&self) {
        if let Some(page) = self.tab_view.selected_page() {
            self.tab_view.close_page(&page);
        }
    }

    /// The tab the context menu was opened on (else the selected one).
    fn menu_page(&self) -> Option<adw::TabPage> {
        self.tab_menu_page
            .take()
            .or_else(|| self.tab_view.selected_page())
    }

    pub(crate) fn close_other_tabs(&self) {
        if let Some(page) = self.menu_page() {
            self.tab_view.set_selected_page(&page);
            self.tab_view.close_other_pages(&page);
        }
    }

    /// Moves a tab into its own new window.
    pub(crate) fn move_tab_to_window(self: &Rc<Self>) {
        let Some(page) = self.menu_page() else {
            return;
        };
        if self.tab_view.n_pages() < 2 {
            return;
        }
        let path = if self.tab_current.borrow().as_ref() == Some(&page) {
            self.path.borrow().clone()
        } else {
            self.tab_data
                .borrow()
                .iter()
                .find(|(p, _)| *p == page)
                .and_then(|(_, d)| d.path.clone())
        };
        self.tab_view.close_page(&page);
        // Still there: its note could not be saved.
        if self.has_page(&page) {
            return;
        }
        if let Some(app) = self
            .window
            .application()
            .and_then(|a| a.downcast::<adw::Application>().ok())
        {
            match path {
                Some(p) => crate::window::build_window_in(&app, self.root.borrow().clone(), p),
                None => crate::window::build_window_empty(&app),
            }
        }
    }

    pub(crate) fn select_tab_index(&self, index: usize) {
        if let Ok(i) = i32::try_from(index)
            && i < self.tab_view.n_pages()
        {
            self.tab_view.set_selected_page(&self.tab_view.nth_page(i));
        }
    }

    /// Selects the tab that shows `path`; false if none does.
    pub(crate) fn select_tab_with(&self, path: &Path) -> bool {
        let page = self
            .tab_data
            .borrow()
            .iter()
            .find(|(_, d)| d.path.as_deref() == Some(path))
            .map(|(p, _)| p.clone());
        match page {
            Some(page) => {
                self.tab_view.set_selected_page(&page);
                true
            }
            None => false,
        }
    }

    /// Opens `path` in a tab of this window: the one that shows it, an empty current one, or a
    /// new one.
    pub(crate) fn open_in_tab(self: &Rc<Self>, path: &Path) {
        if self.path.borrow().as_deref() == Some(path) || self.select_tab_with(path) {
            return;
        }
        let made = self.path.borrow().is_some();
        if made && !self.new_tab() {
            return;
        }
        self.open(path);
        // Unreadable: no empty tab left behind.
        if made && self.path.borrow().as_deref() != Some(path) {
            self.close_tab();
        }
    }

    /// Ctrl+Enter / middle click / Ctrl+Shift+click: `path` beside the current note, as the
    /// preference says (a tab, or a window).
    pub(crate) fn open_beside(self: &Rc<Self>, path: &Path) {
        if self.path.borrow().as_deref() == Some(path) || self.select_tab_with(path) {
            return;
        }
        match self.prefs.borrow().open_files_in {
            OpenFilesIn::Tabs => self.open_in_tab(path),
            OpenFilesIn::Windows => {
                if let Some(app) = self
                    .window
                    .application()
                    .and_then(|a| a.downcast::<adw::Application>().ok())
                {
                    // This window's folder, not the note's parent: a note in a subfolder
                    // keeps its links, backlinks and attachments.
                    crate::window::build_window_in(
                        &app,
                        self.root.borrow().clone(),
                        path.to_path_buf(),
                    );
                }
            }
        }
    }

    /// A note was renamed or moved: the tabs that are not shown follow.
    pub(crate) fn retarget_tabs(&self, moved: impl Fn(&Path) -> Option<PathBuf>) {
        for (page, data) in self.tab_data.borrow_mut().iter_mut() {
            if let Some(new) = data.path.as_deref().and_then(&moved) {
                label(page, Some(&new));
                data.path = Some(new);
            }
        }
    }

    /// A note or folder went to the Trash: its tabs that are not shown close.
    pub(crate) fn close_tabs_under(&self, target: &Path) {
        let pages: Vec<adw::TabPage> = self
            .tab_data
            .borrow()
            .iter()
            .filter(|(_, d)| d.path.as_deref().is_some_and(|n| n.starts_with(target)))
            .map(|(p, _)| p.clone())
            .collect();
        for page in pages {
            self.tab_view.close_page(&page);
        }
    }
}
