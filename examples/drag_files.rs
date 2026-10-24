//! Smoke-test helper (not part of the app): a window whose label drags the files given on the
//! command line as a file list (text/uri-list), like dragging them from Files.
//! `cargo run --example drag_files -- a.md b.txt`

use gtk::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let files: Vec<gtk::gio::File> = std::env::args()
        .skip(1)
        .map(gtk::gio::File::for_path)
        .collect();
    let app = gtk::Application::builder()
        .application_id("com.homielab.MarkdownMate.DragFiles")
        .build();
    app.connect_activate(move |app| {
        let label = gtk::Label::new(Some("drag me"));
        let source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::COPY)
            .build();
        let files = files.clone();
        source.connect_prepare(move |_, _, _| {
            let list = gtk::gdk::FileList::from_array(&files);
            Some(gtk::gdk::ContentProvider::for_value(&list.to_value()))
        });
        label.add_controller(source);
        gtk::ApplicationWindow::builder()
            .application(app)
            .title("drag-files")
            .default_width(200)
            .default_height(200)
            .child(&label)
            .build()
            .present();
    });
    app.run_with_args(&[] as &[&str])
}
