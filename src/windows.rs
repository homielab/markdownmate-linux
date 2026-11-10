use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use crate::editor::Editor;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Present(usize),
    New(PathBuf),
}

/// `shown[i]` = note shown by window i. A note already shown → that window; anything else
/// (another note, a folder) → a new window. Duplicates in `targets` collapse to one route.
pub fn route(targets: &[PathBuf], shown: &[Option<PathBuf>]) -> Vec<Route> {
    let mut routes = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        if !seen.insert(target) {
            continue;
        }
        if let Some(pos) = shown.iter().position(|s| s.as_ref() == Some(target)) {
            routes.push(Route::Present(pos));
        } else {
            routes.push(Route::New(target.clone()));
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

        // three new notes → three New
        assert_eq!(
            route(&[n1.clone(), n2.clone(), n3.clone()], &[]),
            vec![
                Route::New(n1.clone()),
                Route::New(n2.clone()),
                Route::New(n3.clone()),
            ]
        );

        // one already shown → Present(i)
        let shown = vec![Some(n2.clone()), None, Some(n1.clone())];
        assert_eq!(
            route(&[n1.clone(), n3.clone()], &shown),
            vec![Route::Present(2), Route::New(n3.clone())]
        );

        // same note twice → one route
        assert_eq!(
            route(&[n1.clone(), n1.clone()], &shown),
            vec![Route::Present(2)]
        );
        assert_eq!(
            route(&[n3.clone(), n3.clone()], &shown),
            vec![Route::New(n3.clone())]
        );

        // a folder → New
        assert_eq!(
            route(std::slice::from_ref(&folder), &shown),
            vec![Route::New(folder)]
        );
    }
}
