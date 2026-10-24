//! Tìm / thay thế trong note (Ctrl+F, Ctrl+H): thanh trên editor, dùng `SearchContext` của
//! GtkSourceView (không phân biệt hoa thường, vòng lại đầu/cuối, tô sáng mọi kết quả).

use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib::{self, clone};
use sourceview5::prelude::*;

use crate::editor::Editor;
use crate::state;

/// Nhãn cạnh ô tìm: `position` / `count` như `SearchContext` trả (-1 = chưa đếm xong,
/// position 0 = vùng chọn không phải kết quả).
pub(crate) fn match_label(position: i32, count: i32) -> String {
    match (position, count) {
        (_, c) if c < 0 => String::new(),
        (_, 0) => "No results".into(),
        (p, c) if p > 0 => format!("{p} of {c}"),
        (_, 1) => "1 result".into(),
        (_, c) => format!("{c} results"),
    }
}

struct Bar {
    editor: Rc<Editor>,
    widget: gtk::Box,
    find: gtk::Entry,
    replace_row: gtk::Box,
    replace: gtk::Entry,
    label: gtk::Label,
    context: sourceview5::SearchContext,
}

impl Bar {
    /// Vùng chọn hiện tại (kết quả đang chọn), hoặc con trỏ.
    fn bounds(&self) -> (gtk::TextIter, gtk::TextIter) {
        let buffer = &self.editor.buffer;
        buffer.selection_bounds().unwrap_or_else(|| {
            let at = buffer.iter_at_mark(&buffer.get_insert());
            (at, at)
        })
    }

    fn select(&self, found: Option<(gtk::TextIter, gtk::TextIter, bool)>) {
        if let Some((start, end, _)) = found {
            self.editor.buffer.select_range(&start, &end);
            self.editor
                .view
                .scroll_to_mark(&self.editor.buffer.get_insert(), 0.0, true, 0.0, 0.3);
        }
        self.update_label();
    }

    fn update_label(&self) {
        if !self.widget.is_visible() {
            return;
        }
        let count = self.context.occurrences_count();
        let (start, end) = self.bounds();
        let empty = self.find.text().is_empty();
        let text = if empty {
            String::new()
        } else {
            match_label(self.context.occurrence_position(&start, &end), count)
        };
        self.label.set_text(&text);
        if !empty && count == 0 {
            self.find.add_css_class("error");
        } else if count != -1 {
            self.find.remove_css_class("error");
        }
    }

    /// Đặt chuỗi tìm theo ô nhập rồi chọn kết quả gần nhất từ đầu vùng chọn/con trỏ.
    fn search(&self) {
        let text = self.find.text();
        let settings = self.context.settings();
        settings.set_search_text((!text.is_empty()).then_some(text.as_str()));
        if text.is_empty() {
            self.update_label();
            return;
        }
        let found = self.context.forward(&self.bounds().0);
        self.select(found);
    }

    fn next(&self) {
        let found = self.context.forward(&self.bounds().1);
        self.select(found);
    }

    fn previous(&self) {
        let found = self.context.backward(&self.bounds().0);
        self.select(found);
    }

    fn open(&self, replace: bool) {
        let editor = &self.editor;
        if editor.path.borrow().is_none() {
            return;
        }
        if editor.mode.get() == state::Mode::Preview {
            editor.set_mode(state::Mode::Edit);
        }
        if let Some((start, end)) = editor.buffer.selection_bounds()
            && start.line() == end.line()
        {
            self.find.set_text(&editor.buffer.text(&start, &end, false));
        }
        self.widget.set_visible(true);
        self.replace_row.set_visible(replace);
        // Ctrl+H với chuỗi tìm sẵn → gõ ngay chuỗi thay.
        let entry = if replace && !self.find.text().is_empty() {
            &self.replace
        } else {
            &self.find
        };
        entry.grab_focus();
        entry.select_region(0, -1);
        // Sau khi chọn chữ trong ô: ô đó lấy PRIMARY, buffer mất PRIMARY thì bỏ vùng chọn —
        // chọn kết quả sau cùng để vùng chọn (kết quả hiện tại) còn lại.
        self.search();
    }

    fn close(&self) {
        self.widget.set_visible(false);
        // Bỏ tô sáng và không đếm kết quả nữa khi thanh đóng.
        self.context.settings().set_search_text(None);
        if self.editor.mode.get() != state::Mode::Preview {
            self.editor.view.grab_focus();
        }
    }

    /// Find in Preview, when the keys belong to it: Preview mode, or side by side with the
    /// focus in the preview pane (page or its find bar); `open` also needs its bar shown.
    fn preview_find(&self, open: bool) -> Option<&crate::previewfind::Bar> {
        let editor = &self.editor;
        let find = editor.preview_find.get()?;
        let wanted = match editor.mode.get() {
            state::Mode::Preview => editor.path.borrow().is_some(),
            state::Mode::Split => {
                let pane = editor.panes.end_child()?;
                gtk::prelude::GtkWindowExt::focus(&editor.window)
                    .is_some_and(|w| w == pane || w.is_ancestor(&pane))
            }
            state::Mode::Edit => false,
        };
        (wanted && (!open || find.is_open())).then_some(find.as_ref())
    }

    fn replace_one(&self) {
        let (mut start, mut end) = self.bounds();
        if self.context.occurrence_position(&start, &end) > 0 {
            let text = self.replace.text();
            if let Err(e) = self.context.replace(&mut start, &mut end, &text) {
                self.editor.toast(&format!("Cannot replace: {e}"));
                return;
            }
            self.editor.buffer.place_cursor(&end);
        }
        self.next();
    }

    fn replace_all(&self) {
        // Một bước undo: GtkSourceView bọc replace_all trong một user action.
        if let Err(e) = self.context.replace_all(&self.replace.text()) {
            self.editor.toast(&format!("Cannot replace: {e}"));
        }
        self.update_label();
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(["flat"])
        .build()
}

/// Dựng thanh tìm ở đầu `pane` (khung editor) và các action `win.find`, `win.replace`,
/// `win.find-next`, `win.find-previous` với phím tắt.
pub(crate) fn find_bar(editor: &Rc<Editor>, pane: &gtk::Box, app: &adw::Application) {
    let settings = sourceview5::SearchSettings::builder()
        .case_sensitive(false)
        .wrap_around(true)
        .build();
    let context = sourceview5::SearchContext::new(&editor.buffer, Some(&settings));
    let find = gtk::Entry::builder()
        .placeholder_text("Find")
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
    let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for w in [
        find.upcast_ref::<gtk::Widget>(),
        label.upcast_ref(),
        prev.upcast_ref(),
        next.upcast_ref(),
        close.upcast_ref(),
    ] {
        find_row.append(w);
    }
    let replace = gtk::Entry::builder()
        .placeholder_text("Replace")
        .hexpand(true)
        .build();
    let replace_btn = gtk::Button::with_label("Replace");
    let replace_all_btn = gtk::Button::builder()
        .label("Replace All")
        .tooltip_text("Replace All (Ctrl+Enter)")
        .build();
    let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    replace_row.append(&replace);
    replace_row.append(&replace_btn);
    replace_row.append(&replace_all_btn);
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .visible(false)
        .build();
    widget.append(&find_row);
    widget.append(&replace_row);
    pane.prepend(&widget);

    let bar = Rc::new(Bar {
        editor: editor.clone(),
        widget,
        find,
        replace_row,
        replace,
        label,
        context,
    });

    bar.find.connect_changed(clone!(
        #[weak]
        bar,
        move |_| bar.search()
    ));
    bar.context.connect_occurrences_count_notify(clone!(
        #[weak]
        bar,
        move |_| bar.update_label()
    ));
    editor.buffer.connect_mark_set(clone!(
        #[weak]
        bar,
        move |buffer, _, mark| {
            if *mark == buffer.get_insert() {
                bar.update_label();
            }
        }
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
        move |_| bar.close()
    ));
    replace_btn.connect_clicked(clone!(
        #[weak]
        bar,
        move |_| bar.replace_one()
    ));
    replace_all_btn.connect_clicked(clone!(
        #[weak]
        bar,
        move |_| bar.replace_all()
    ));

    // Esc ở bất kỳ đâu trong thanh; Enter / Shift+Enter ở ô tìm; Enter / Ctrl+Enter ở ô thay.
    // (Entry không tự has_focus — focus nằm ở GtkText con — nên mỗi ô một controller.)
    let on_key = |entry: &gtk::Entry, run: fn(&Bar, gdk::ModifierType)| {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(clone!(
            #[weak]
            bar,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| match key {
                gdk::Key::Escape => {
                    bar.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                    run(&bar, modifiers);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        ));
        entry.add_controller(keys);
    };
    on_key(&bar.find, |b, m| {
        if m.contains(gdk::ModifierType::SHIFT_MASK) {
            b.previous();
        } else {
            b.next();
        }
    });
    on_key(&bar.replace, |b, m| {
        if m.contains(gdk::ModifierType::CONTROL_MASK) {
            b.replace_all();
        } else {
            b.replace_one();
        }
    });

    // Ctrl+F/H/G không bị GTK hay GtkSourceView dùng; Shift+Ctrl+F là Find in Folder.
    // In Preview, or side by side with the focus in the preview, Ctrl+F / Ctrl+G go to Find
    // in Preview (`previewfind`) instead; Ctrl+H always edits.
    let window = &editor.window;
    for (name, accels, run) in [
        (
            "find",
            &["<Control>f"][..],
            (|b: &Bar| match b.preview_find(false) {
                Some(p) => p.open(),
                None => b.open(false),
            }) as fn(&Bar),
        ),
        ("replace", &["<Control>h"][..], |b| b.open(true)),
        ("find-next", &["<Control>g"][..], |b| {
            if let Some(p) = b.preview_find(true) {
                p.next();
            } else if b.widget.is_visible() {
                b.next();
            }
        }),
        ("find-previous", &["<Shift><Control>g"][..], |b| {
            if let Some(p) = b.preview_find(true) {
                p.previous();
            } else if b.widget.is_visible() {
                b.previous();
            }
        }),
    ] {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(clone!(
            #[strong]
            bar,
            move |_, _| run(&bar)
        ));
        window.add_action(&action);
        app.set_accels_for_action(&format!("win.{name}"), accels);
    }
}

#[cfg(test)]
mod tests {
    use super::match_label;

    #[test]
    fn labels() {
        assert_eq!(match_label(2, 5), "2 of 5");
        assert_eq!(match_label(0, 0), "No results");
        assert_eq!(match_label(-1, -1), "");
        assert_eq!(match_label(0, 3), "3 results");
        assert_eq!(match_label(0, 1), "1 result");
        assert_eq!(match_label(-1, 4), "4 results");
    }
}
