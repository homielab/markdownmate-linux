//! Find in the preview (Ctrl+F in Preview, or side by side with the preview focused): a find
//! bar above the rendered note, searched with WebKit's `FindController` (case-insensitive,
//! wrap-around, every match highlighted). No Replace: the page is read-only.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk::glib::{self, clone};
use webkit6::prelude::*;

/// Matches counted at most; past it the label says "1000+ results".
const MAX_MATCHES: u32 = 1000;

/// Label beside the field for `count` matches (`None`: nothing searched).
pub(crate) fn count_label(count: Option<u32>) -> String {
    match count {
        None => String::new(),
        Some(0) => "No results".into(),
        Some(1) => "1 result".into(),
        Some(c) if c > MAX_MATCHES => format!("{MAX_MATCHES}+ results"),
        Some(c) => format!("{c} results"),
    }
}

pub(crate) struct Bar {
    widget: gtk::Box,
    entry: gtk::Entry,
    label: gtk::Label,
    preview: webkit6::WebView,
    controller: webkit6::FindController,
    /// The field's text was searched since the bar opened (a step can continue it).
    searched: Cell<bool>,
    /// Steps whose result has not come back: their `found-text` reports a count of 1, so
    /// only a new search's count goes in the label (`count_matches` would clear the
    /// highlights). Results come back in order.
    steps: Cell<u32>,
}

impl Bar {
    fn options() -> u32 {
        (webkit6::FindOptions::CASE_INSENSITIVE | webkit6::FindOptions::WRAP_AROUND).bits()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    /// The keyboard is in the search field (its text widget inside the entry).
    pub(crate) fn has_focus(&self) -> bool {
        self.is_open()
            && self
                .entry
                .root()
                .and_then(|root| root.focus())
                .is_some_and(|focus| focus.is_ancestor(&self.entry) || focus == self.entry)
    }

    /// Shows the bar with the field focused and its text selected; Enter searches it again.
    /// Not searched now: the page selecting the match takes PRIMARY, which would drop the
    /// field's selection (typing would then add to the old text instead of replacing it).
    pub(crate) fn open(&self) {
        self.widget.set_visible(true);
        self.searched.set(false);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// Hides the bar and clears the highlights; `focus` puts the keyboard back on the page.
    pub(crate) fn close(&self, focus: bool) {
        if !self.is_open() {
            return;
        }
        self.widget.set_visible(false);
        self.searched.set(false);
        self.steps.set(0);
        self.controller.search_finish();
        self.label.set_text("");
        self.entry.remove_css_class("error");
        if focus {
            self.preview.grab_focus();
        }
    }

    /// Searches the field's text from the top (or the page's selection), highlighting all.
    fn search(&self) {
        let text = self.entry.text();
        if text.is_empty() {
            self.controller.search_finish();
            self.show(None);
        } else {
            self.searched.set(true);
            self.steps.set(0);
            self.controller.search(&text, Self::options(), MAX_MATCHES);
        }
    }

    /// Next / previous match (on a re-rendered page this also restores the highlights); the
    /// first step after opening searches the field's text again.
    pub(crate) fn next(&self) {
        if !self.searched.get() {
            self.search();
        } else if !self.entry.text().is_empty() {
            self.steps.set(self.steps.get() + 1);
            self.controller.search_next();
        }
    }

    pub(crate) fn previous(&self) {
        if !self.searched.get() {
            self.search();
        } else if !self.entry.text().is_empty() {
            self.steps.set(self.steps.get() + 1);
            self.controller.search_previous();
        }
    }

    /// The page was re-rendered or another note loaded: recount the matches (the new page has
    /// no highlights until the next search step; searching now would scroll the page).
    pub(crate) fn refresh(&self) {
        let text = self.entry.text();
        if self.is_open() && self.searched.get() && !text.is_empty() {
            self.controller
                .count_matches(&text, Self::options(), MAX_MATCHES);
        }
    }

    fn show(&self, count: Option<u32>) {
        self.label.set_text(&count_label(count));
        if count == Some(0) {
            self.entry.add_css_class("error");
        } else {
            self.entry.remove_css_class("error");
        }
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(["flat"])
        .build()
}

/// Builds the bar at the top of `pane` (the box holding `preview`); hidden until opened.
pub(crate) fn find_bar(preview: &webkit6::WebView, pane: &gtk::Box) -> Rc<Bar> {
    let entry = gtk::Entry::builder()
        .placeholder_text("Find in Preview")
        .primary_icon_name("system-search-symbolic")
        .hexpand(true)
        .build();
    let label = gtk::Label::builder()
        .width_chars(10)
        .css_classes(["caption", "dim-label"])
        .build();
    let prev = icon_button("go-up-symbolic", "Previous (Shift+Enter)");
    let next = icon_button("go-down-symbolic", "Next (Enter)");
    let close = icon_button("window-close-symbolic", "Close (Esc)");
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .visible(false)
        .build();
    for w in [
        entry.upcast_ref::<gtk::Widget>(),
        label.upcast_ref(),
        prev.upcast_ref(),
        next.upcast_ref(),
        close.upcast_ref(),
    ] {
        widget.append(w);
    }
    pane.prepend(&widget);

    let controller = preview
        .find_controller()
        .expect("a web view has a find controller");
    let bar = Rc::new(Bar {
        widget,
        entry,
        label,
        preview: preview.clone(),
        controller,
        searched: Cell::new(false),
        steps: Cell::new(0),
    });

    // Signals for a search the bar no longer shows (closed, field emptied) are ignored.
    bar.controller.connect_found_text(clone!(
        #[weak]
        bar,
        move |_, count| {
            let steps = bar.steps.get();
            bar.steps.set(steps.saturating_sub(1));
            if steps == 0 && bar.is_open() && !bar.entry.text().is_empty() {
                bar.show(Some(count));
            }
        }
    ));
    bar.controller.connect_counted_matches(clone!(
        #[weak]
        bar,
        move |_, count| {
            if bar.is_open() && !bar.entry.text().is_empty() {
                bar.show(Some(count));
            }
        }
    ));
    bar.controller.connect_failed_to_find_text(clone!(
        #[weak]
        bar,
        move |_| {
            bar.steps.set(bar.steps.get().saturating_sub(1));
            if bar.is_open() && !bar.entry.text().is_empty() {
                bar.show(Some(0));
            }
        }
    ));
    bar.entry.connect_changed(clone!(
        #[weak]
        bar,
        move |_| bar.search()
    ));
    prev.connect_clicked(clone!(
        #[weak]
        bar,
        move |_| bar.previous()
    ));
    next.connect_clicked(clone!(
        #[weak]
        bar,
        move |_| bar.next()
    ));
    close.connect_clicked(clone!(
        #[weak]
        bar,
        move |_| bar.close(true)
    ));

    // Esc closes; Enter / Shift+Enter step. The entry's focus is in its GtkText child, so
    // the controller captures on the entry.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(clone!(
        #[weak]
        bar,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| match key {
            gdk::Key::Escape => {
                bar.close(true);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    bar.previous();
                } else {
                    bar.next();
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    ));
    bar.entry.add_controller(keys);
    // Esc on the page itself closes the bar too.
    let page_keys = gtk::EventControllerKey::new();
    page_keys.connect_key_pressed(clone!(
        #[weak]
        bar,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            if key == gdk::Key::Escape && bar.is_open() {
                bar.close(true);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    ));
    preview.add_controller(page_keys);
    bar
}

#[cfg(test)]
mod tests {
    use super::count_label;

    #[test]
    fn labels() {
        assert_eq!(count_label(None), "");
        assert_eq!(count_label(Some(0)), "No results");
        assert_eq!(count_label(Some(1)), "1 result");
        assert_eq!(count_label(Some(42)), "42 results");
        assert_eq!(count_label(Some(1000)), "1000 results");
        assert_eq!(count_label(Some(u32::MAX)), "1000+ results");
    }
}
