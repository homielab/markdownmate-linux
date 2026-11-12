use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use crate::editor::Editor;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Tab `.1` of window `.0` shows the note: select it and present the window.
    Present(usize, usize),
    /// Open the note in a new tab of window `.0`.
    Tab(usize, PathBuf),
    /// A new window on `.0` (a note or a folder), then the notes `.1` as further tabs of it.
    New(PathBuf, Vec<PathBuf>),
}

/// An open window, for `route`: its folder and the notes its tabs show.
#[derive(Debug, Default, Clone)]
pub struct Shown {
    pub folder: Option<PathBuf>,
    pub tabs: Vec<Option<PathBuf>>,
}

/// Where each of `targets` (files and folders passed to the app) goes. A note already shown →
/// that tab. With `tabs` (Open files in = Tabs) another note → a new tab of a window whose
/// folder holds it (the `active` window first), or of the window this call opens on its
/// folder; so three notes of one folder give one window with three tabs, also when no window
/// is open yet. Anything else (a note with Windows, a note of a folder no window has, a
/// folder) → a new window. Duplicates in `targets` collapse to one route.
pub fn route(
    targets: &[PathBuf],
    windows: &[Shown],
    active: Option<usize>,
    tabs: bool,
    is_dir: impl Fn(&Path) -> bool,
) -> Vec<Route> {
    let holds = |w: usize, target: &Path| {
        windows
            .get(w)
            .and_then(|s| s.folder.as_deref())
            .is_some_and(|folder| target.starts_with(folder))
    };
    let mut routes = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        if !seen.insert(target) {
            continue;
        }
        let found = windows.iter().enumerate().find_map(|(w, shown)| {
            let t = shown.tabs.iter().position(|s| s.as_ref() == Some(target))?;
            Some((w, t))
        });
        if let Some((w, t)) = found {
            routes.push(Route::Present(w, t));
            continue;
        }
        if !tabs || is_dir(target) {
            routes.push(Route::New(target.clone(), Vec::new()));
            continue;
        }
        let open = active
            .filter(|w| holds(*w, target))
            .or_else(|| (0..windows.len()).find(|w| holds(*w, target)));
        if let Some(w) = open {
            routes.push(Route::Tab(w, target.clone()));
            continue;
        }
        // A window this call opens: its folder is the folder given, or the note's own.
        let new = routes.iter_mut().find_map(|r| match r {
            Route::New(first, more) => {
                let folder = if is_dir(first) {
                    Some(first.as_path())
                } else {
                    first.parent()
                };
                folder
                    .is_some_and(|f| target.starts_with(f))
                    .then_some(more)
            }
            _ => None,
        });
        match new {
            Some(more) => more.push(target.clone()),
            None => routes.push(Route::New(target.clone(), Vec::new())),
        }
    }
    routes
}

thread_local! {
    static OPEN: RefCell<Vec<Weak<Editor>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn register(editor: &Rc<Editor>) {
    OPEN.with(|open| {
        open.borrow_mut().push(Rc::downgrade(editor));
    });
}

pub(crate) fn unregister(editor: &Editor) {
    OPEN.with(|open| {
        open.borrow_mut().retain(|w| match w.upgrade() {
            Some(e) => !std::ptr::eq(Rc::as_ptr(&e), editor),
            None => false,
        });
    });
}

pub(crate) fn live() -> Vec<Rc<Editor>> {
    OPEN.with(|open| {
        let mut list = open.borrow_mut();
        let live: Vec<Rc<Editor>> = list.iter().filter_map(Weak::upgrade).collect();
        list.retain(|w| w.strong_count() > 0);
        live
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_rules() {
        let n1 = PathBuf::from("/notes/1.md");
        let n2 = PathBuf::from("/notes/2.md");
        let n3 = PathBuf::from("/notes/3.md");
        let folder = PathBuf::from("/notes/folder");
        let is_dir = |p: &Path| p == Path::new("/notes/folder");
        let new = |p: &PathBuf| Route::New(p.clone(), Vec::new());
        let three = [n1.clone(), n2.clone(), n3.clone()];

        // no window yet, Windows → three windows
        assert_eq!(
            route(&three, &[], None, false, is_dir),
            vec![new(&n1), new(&n2), new(&n3)]
        );
        // no window yet, Tabs → one window with three tabs
        assert_eq!(
            route(&three, &[], None, true, is_dir),
            vec![Route::New(n1.clone(), vec![n2.clone(), n3.clone()])]
        );
        // … one window per folder
        let far = PathBuf::from("/other/9.md");
        assert_eq!(
            route(
                &[n1.clone(), far.clone(), n2.clone()],
                &[],
                None,
                true,
                is_dir
            ),
            vec![Route::New(n1.clone(), vec![n2.clone()]), new(&far)]
        );
        // a folder, then a note in it → its tab
        let inner = PathBuf::from("/notes/folder/in.md");
        assert_eq!(
            route(&[folder.clone(), inner.clone()], &[], None, true, is_dir),
            vec![Route::New(folder.clone(), vec![inner.clone()])]
        );

        // one already shown (any tab of any window) → Present(window, tab)
        let shown = vec![
            Shown {
                folder: Some("/notes".into()),
                tabs: vec![Some(n2.clone())],
            },
            Shown::default(),
            Shown {
                folder: Some("/notes".into()),
                tabs: vec![Some(n3.clone()), Some(n1.clone())],
            },
        ];
        assert_eq!(
            route(&[n1.clone(), n2.clone()], &shown, None, false, is_dir),
            vec![Route::Present(2, 1), Route::Present(0, 0)]
        );

        // Windows: another note → New; Tabs: → a new tab of a window on its folder, the
        // active one first
        let other = PathBuf::from("/notes/sub/4.md");
        assert_eq!(
            route(std::slice::from_ref(&other), &shown, Some(2), false, is_dir),
            vec![new(&other)]
        );
        assert_eq!(
            route(&[other.clone(), n1.clone()], &shown, Some(2), true, is_dir),
            vec![Route::Tab(2, other.clone()), Route::Present(2, 1)]
        );
        // the active window has another folder (or none, or is gone) → the first that holds it
        for active in [Some(1), Some(7), None] {
            assert_eq!(
                route(std::slice::from_ref(&other), &shown, active, true, is_dir),
                vec![Route::Tab(0, other.clone())]
            );
        }
        // a note of a folder no window has → its own window, never a tab of another folder
        assert_eq!(
            route(std::slice::from_ref(&far), &shown, Some(0), true, is_dir),
            vec![new(&far)]
        );

        // same note twice → one route
        assert_eq!(
            route(&[n1.clone(), n1.clone()], &shown, None, false, is_dir),
            vec![Route::Present(2, 1)]
        );
        assert_eq!(
            route(
                &[other.clone(), other.clone()],
                &shown,
                Some(0),
                true,
                is_dir
            ),
            vec![Route::Tab(0, other)]
        );

        // a folder → New, also with Tabs
        assert_eq!(
            route(std::slice::from_ref(&folder), &shown, Some(0), true, is_dir),
            vec![new(&folder)]
        );
    }
}
