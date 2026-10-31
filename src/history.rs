//! Back / Forward (Alt+Left / Alt+Right): the notes one window showed, in order, like a
//! browser's history. `Editor::open` records every note it shows (sidebar, links, Open
//! Quickly, backlinks, Today's Note…) with `visit`, and Back / Forward with `move_to`. Pure.

use std::path::{Path, PathBuf};

/// The most entries kept; the oldest go first.
pub const LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

#[derive(Debug, Default)]
pub struct History {
    /// Notes, oldest first.
    entries: Vec<PathBuf>,
    /// The entry shown now; None while empty.
    index: Option<usize>,
}

impl History {
    pub fn entries(&self) -> &[PathBuf] {
        &self.entries
    }

    pub fn current(&self) -> Option<&Path> {
        self.entries.get(self.index?).map(PathBuf::as_path)
    }

    /// A note shown by anything but Back / Forward: it becomes the current entry and the
    /// entries after the old current one are dropped. The current note shown again adds nothing.
    pub fn visit(&mut self, path: &Path) {
        if self.current() == Some(path) {
            return;
        }
        self.entries.truncate(self.index.map_or(0, |i| i + 1));
        self.entries.push(path.to_path_buf());
        if self.entries.len() > LIMIT {
            self.entries.drain(..self.entries.len() - LIMIT);
        }
        self.index = Some(self.entries.len() - 1);
    }

    /// The nearest entry in `direction` whose note `exists` (notes deleted on disk are skipped)
    /// and that isn't the current note again; None when there is nothing to go to.
    pub fn target(&self, direction: Direction, exists: impl Fn(&Path) -> bool) -> Option<usize> {
        let index = self.index?;
        let current = self.current();
        let found = |&i: &usize| {
            let p = self.entries[i].as_path();
            Some(p) != current && exists(p)
        };
        match direction {
            Direction::Back => (0..index).rev().find(found),
            Direction::Forward => (index + 1..self.entries.len()).find(found),
        }
    }

    /// Back / Forward reached entry `index`: it becomes the current one, nothing is dropped.
    pub fn move_to(&mut self, index: usize) {
        if index < self.entries.len() {
            self.index = Some(index);
        }
    }

    /// Notes renamed or moved: each entry `moved` maps to a new path follows it.
    pub fn retarget(&mut self, moved: impl Fn(&Path) -> Option<PathBuf>) {
        for entry in &mut self.entries {
            if let Some(new) = moved(entry) {
                *entry = new;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/notes/{name}.md"))
    }

    fn all(_: &Path) -> bool {
        true
    }

    /// Back / Forward as `Editor::go` does it: the target, then `move_to`.
    fn go(h: &mut History, direction: Direction) -> Option<PathBuf> {
        let i = h.target(direction, all)?;
        h.move_to(i);
        h.current().map(Path::to_path_buf)
    }

    #[test]
    fn back_and_forward_like_a_browser() {
        let mut h = History::default();
        assert_eq!(h.target(Direction::Back, all), None);
        h.visit(&p("a"));
        h.visit(&p("b"));
        h.visit(&p("c"));
        assert_eq!(h.target(Direction::Forward, all), None);
        assert_eq!(go(&mut h, Direction::Back), Some(p("b")));
        assert_eq!(go(&mut h, Direction::Back), Some(p("a")));
        assert_eq!(h.target(Direction::Back, all), None);
        assert_eq!(go(&mut h, Direction::Forward), Some(p("b")));
        // Opening D from B drops C.
        h.visit(&p("d"));
        assert_eq!(h.entries(), [p("a"), p("b"), p("d")]);
        assert_eq!(h.target(Direction::Forward, all), None);
        assert_eq!(go(&mut h, Direction::Back), Some(p("b")));
    }

    #[test]
    fn the_current_note_again_adds_nothing() {
        let mut h = History::default();
        h.visit(&p("a"));
        h.visit(&p("a"));
        h.visit(&p("b"));
        h.visit(&p("b"));
        assert_eq!(h.entries(), [p("a"), p("b")]);
    }

    #[test]
    fn keeps_the_last_fifty() {
        let mut h = History::default();
        for n in 0..LIMIT + 7 {
            h.visit(&p(&n.to_string()));
        }
        assert_eq!(h.entries().len(), LIMIT);
        assert_eq!(h.entries()[0], p("7"));
        assert_eq!(h.current(), Some(p(&(LIMIT + 6).to_string()).as_path()));
        assert_eq!(h.target(Direction::Back, all), Some(LIMIT - 2));
    }

    #[test]
    fn skips_deleted_notes_and_the_current_one() {
        let mut h = History::default();
        for name in ["a", "b", "c", "b", "d"] {
            h.visit(&p(name));
        }
        // c deleted on disk. From d, Back is the nearest b.
        let gone = |path: &Path| path != p("c");
        assert_eq!(h.target(Direction::Back, gone), Some(3));
        h.move_to(3);
        // From b: c is gone, the b at index 1 is the current note again, so a.
        assert_eq!(h.target(Direction::Back, gone), Some(0));
        assert_eq!(h.target(Direction::Forward, gone), Some(4));
        let none = |_: &Path| false;
        assert_eq!(h.target(Direction::Back, none), None);
        assert_eq!(h.target(Direction::Forward, none), None);
    }

    #[test]
    fn renames_and_moves_are_followed() {
        let mut h = History::default();
        h.visit(&p("a"));
        h.visit(&PathBuf::from("/notes/sub/b.md"));
        h.visit(&p("c"));
        h.retarget(|path| crate::editor::moved(path, Path::new("/notes/sub"), Path::new("/x")));
        h.retarget(|path| crate::editor::moved(path, &p("a"), &p("z")));
        assert_eq!(h.entries(), [p("z"), PathBuf::from("/x/b.md"), p("c")]);
        assert_eq!(h.current(), Some(p("c").as_path()));
    }
}
