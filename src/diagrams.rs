//! Mermaid diagrams in a WebView: the bundled Mermaid (`resources/mermaid/`) and
//! `diagrams.js`, installed into the app's script world only on pages that have a
//! ```mermaid block (`render::has_diagrams`). The page's own scripts stay off; Mermaid runs
//! in `strict` mode (labels sanitized) and loads nothing from the network.

use std::sync::OnceLock;

use gtk::gio;
use webkit6::prelude::*;

use crate::SCRIPT_WORLD;

/// Mermaid 11 (MIT), embedded so the preview works offline and needs no installed files.
const MERMAID: &str = include_str!("../resources/mermaid/mermaid.min.js");
/// The app's side: `window.mmDiagrams` (`apply`, `take`, `run`, `finish`).
const SCRIPT: &str = include_str!("diagrams.js");

/// Draws the page's diagrams (if any), then `done(true)`; `done(false)` when the page has no
/// diagram, a newer run took over, or the script could not run (the blocks stay code).
const RUN: &str = "if (!document.querySelector('div.mermaid')) return 'none';\
if (!window.mmDiagrams) return 'missing';\
return String(await window.mmDiagrams.run());";

/// Script for `swap_page`: takes the old body's diagrams before the body is replaced
/// (`TAKE`) and puts them back in the new one (`APPLY`), so diagrams don't flicker while typing.
pub(crate) const TAKE: &str = "var mmOld=window.mmDiagrams?window.mmDiagrams.take():null;";
pub(crate) const APPLY: &str = "if(window.mmDiagrams)window.mmDiagrams.apply(mmOld);";

/// Lets `view` keep globals in the app's script world between scripts (Mermaid, `mmDiagrams`):
/// WebKitGTK drops a named world, and everything defined in it, as soon as nothing holds it,
/// i.e. after each `evaluate_javascript`. A user script in the world holds it for the view.
pub(crate) fn keep_world(view: &webkit6::WebView) {
    if let Some(manager) = view.user_content_manager() {
        manager.add_script(&webkit6::UserScript::for_world(
            "0",
            webkit6::UserContentInjectedFrames::TopFrame,
            webkit6::UserScriptInjectionTime::End,
            SCRIPT_WORLD,
            &[],
            &[],
        ));
    }
}

/// Draws the diagrams of the page shown in `view`, installing Mermaid first if the page has
/// diagrams and it is not there yet; `done(drawn)` runs once they are all drawn (or not).
pub(crate) fn draw(view: &webkit6::WebView, done: impl FnOnce(bool) + 'static) {
    let target = view.clone();
    run(view, move |state| {
        if state != "missing" {
            return done(state == "true");
        }
        let view = target.clone();
        target.evaluate_javascript(
            install_script(),
            Some(SCRIPT_WORLD),
            Some("markdownmate:///mermaid.min.js"),
            gio::Cancellable::NONE,
            move |result| match result {
                Ok(_) => run(&view, move |state| done(state == "true")),
                Err(e) => {
                    eprintln!("markdownmate: could not load the diagram script: {e}");
                    done(false)
                }
            },
        );
    });
}

/// Export as HTML: the page with its diagrams drawn as static SVG (no script), the code of
/// drawn diagrams removed. Call after `draw`.
pub(crate) fn finished_page(view: &webkit6::WebView, done: impl FnOnce(Option<String>) + 'static) {
    view.evaluate_javascript(
        "window.mmDiagrams?window.mmDiagrams.finish():null",
        Some(SCRIPT_WORLD),
        None,
        gio::Cancellable::NONE,
        move |result| {
            let html = result
                .ok()
                .filter(|v| v.is_string())
                .map(|v| v.to_str().to_string());
            done(html)
        },
    );
}

fn run(view: &webkit6::WebView, done: impl FnOnce(&str) + 'static) {
    view.call_async_javascript_function(
        RUN,
        None,
        Some(SCRIPT_WORLD),
        None,
        gio::Cancellable::NONE,
        move |result| match result {
            Ok(v) => done(&v.to_str()),
            Err(e) => {
                eprintln!("markdownmate: could not draw diagrams: {e}");
                done("error")
            }
        },
    );
}

/// Mermaid followed by `diagrams.js`, built once (Mermaid is ~3.5 MB).
fn install_script() -> &'static str {
    static SCRIPT_TEXT: OnceLock<String> = OnceLock::new();
    SCRIPT_TEXT.get_or_init(|| format!("{MERMAID}\n;{SCRIPT}\n;0"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn mermaid_is_bundled() {
        assert!(super::MERMAID.contains("11.17.2"));
        assert!(super::MERMAID.trim_end().ends_with(".default;"));
        assert!(super::SCRIPT.contains("securityLevel: 'strict'"));
    }
}
