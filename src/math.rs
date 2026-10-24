//! Formulas typeset by KaTeX in a WebView: the bundled KaTeX (`resources/katex/`, fonts
//! embedded in its style sheet) and `math.js`, installed into the app's script world only on
//! pages that have a formula (`render::has_math`). The page's own scripts stay off and nothing
//! loads from the network. Without it (no script, Copy as HTML) formulas stay MathML.

use std::sync::OnceLock;

use gtk::gio;
use webkit6::prelude::*;

use crate::SCRIPT_WORLD;
use crate::render::js_string;

/// KaTeX 0.18 (MIT), embedded so the preview works offline and needs no installed files.
const KATEX: &str = include_str!("../resources/katex/katex.min.js");
/// KaTeX's style sheet with its fonts as `data:` URLs (`resources/katex/inline-fonts.py`).
const CSS: &str = include_str!("../resources/katex/katex.min.css");
/// The app's side: `window.mmMath` (`take`, `render`, `run`, `finish`).
const SCRIPT: &str = include_str!("math.js");

/// Typesets the page's formulas (if any) and waits for the fonts, then `true`; `none` when
/// the page has no formula, `missing` when KaTeX is not installed yet.
const RUN: &str = "if (!document.querySelector('span.math[data-tex]')) return 'none';\
if (!window.mmMath) return 'missing';\
return String(await window.mmMath.run());";

/// Script for `swap_page`: takes the old body's typeset formulas before the body is replaced
/// (`TAKE`) and puts them back in the new one (`APPLY`), so formulas don't flicker while typing.
pub(crate) const TAKE: &str = "var mmMathOld=window.mmMath?window.mmMath.take():null;";
pub(crate) const APPLY: &str = "if(window.mmMath)window.mmMath.render(mmMathOld);";

/// Script for Export as HTML, before the page is read: KaTeX's style sheet into the head.
pub(crate) const FINISH: &str = "if(window.mmMath)window.mmMath.finish();";

/// Typesets the formulas of the page shown in `view`, installing KaTeX first if the page has
/// formulas and it is not there yet; `done(typeset)` runs once they are typeset (or not).
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
            Some("markdownmate:///katex.min.js"),
            gio::Cancellable::NONE,
            move |result| match result {
                Ok(_) => run(&view, move |state| done(state == "true")),
                Err(e) => {
                    eprintln!("markdownmate: could not load the math script: {e}");
                    done(false)
                }
            },
        );
    });
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
                eprintln!("markdownmate: could not typeset formulas: {e}");
                done("error")
            }
        },
    );
}

/// KaTeX, its style sheet (handed to `math.js` in `window.mmKatexCss`), then `math.js`; built
/// once.
fn install_script() -> &'static str {
    static SCRIPT_TEXT: OnceLock<String> = OnceLock::new();
    SCRIPT_TEXT.get_or_init(|| {
        format!(
            "{KATEX}\n;window.mmKatexCss={};\n{SCRIPT}\n;0",
            js_string(CSS)
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn katex_is_bundled() {
        assert!(super::KATEX.contains("version:\"0.18.9\""));
        // Fonts embedded, none left to load from files.
        assert!(super::CSS.contains("font-family:KaTeX_Main"));
        assert!(super::CSS.contains("url(data:font/woff2;base64,"));
        assert!(!super::CSS.contains("url(fonts/"));
        assert!(super::SCRIPT.contains("throwOnError: true"));
        assert!(super::install_script().contains("window.mmKatexCss=\"@font-face{"));
    }
}
