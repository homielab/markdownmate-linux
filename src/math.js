// Typesets the page's formulas with KaTeX: `render.rs` writes each one as
// <span class="math[ display]" data-delim="$" data-tex="source">MathML</span>. Runs in the
// app's script world (never the page's), right after the bundled KaTeX, once per page;
// `math.rs` drives it. The MathML stays until KaTeX replaces it; a formula KaTeX rejects
// shows its source with the error style (`code.math-error`, the message as its tooltip).
// Results are cached by source, and `take` / `render` move the old body's typeset formulas
// into the new one, so a re-render while typing only typesets the formula that changed.
(function () {
  'use strict';
  if (window.mmMath) return;
  const css = window.mmKatexCss;
  delete window.mmKatexCss;
  const done = new Map(); // key → HTML
  const typeset = new WeakSet(); // span.math showing KaTeX's result for its own source
  let sheet = null;

  const formulas = () => Array.from(document.querySelectorAll('span.math[data-tex]'));
  const keyOf = (span) => (span.classList.contains('display') ? 'D' : 'I') + span.dataset.tex;

  // KaTeX's style sheet (fonts embedded) as a sheet of the document, not an element of its
  // head, so it survives the preview replacing the head; a <style> where that can't be done.
  function addStyle() {
    if (sheet) return;
    try {
      sheet = new CSSStyleSheet();
      sheet.replaceSync(css);
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    } catch (e) {
      sheet = null;
      if (document.getElementById('mm-katex')) return;
      const style = document.createElement('style');
      style.id = 'mm-katex';
      style.textContent = css;
      document.head.append(style);
    }
  }

  function html(span) {
    const tex = span.dataset.tex;
    const display = span.classList.contains('display');
    try {
      return katex.renderToString(tex, { displayMode: display, throwOnError: true });
    } catch (error) {
      const delim = span.dataset.delim || '';
      const code = document.createElement('code');
      code.className = 'math-error';
      code.textContent = delim + tex + delim;
      code.title = String((error && error.message) || error);
      return code.outerHTML;
    }
  }

  // The typeset formulas of the body about to be replaced, by key, in page order.
  function take() {
    const old = new Map();
    for (const span of formulas()) {
      if (!typeset.has(span)) continue;
      const key = keyOf(span);
      if (!old.has(key)) old.set(key, []);
      old.get(key).push(span);
    }
    return old;
  }

  // Typesets every formula not showing KaTeX's result yet: moved over from `previous` (what
  // `take` returned before the body was replaced), from the cache, or typeset now.
  function render(previous) {
    addStyle();
    const keep = new Set();
    for (const span of formulas()) {
      const key = keyOf(span);
      keep.add(key);
      if (typeset.has(span)) continue;
      const old = previous && previous.get(key);
      if (old && old.length) {
        span.replaceChildren(...old.shift().childNodes);
      } else {
        let result = done.get(key);
        if (result === undefined) {
          result = html(span);
          done.set(key, result);
        }
        span.innerHTML = result;
      }
      typeset.add(span);
    }
    if (done.size > 2 * keep.size + 256) {
      for (const key of done.keys()) if (!keep.has(key)) done.delete(key);
    }
  }

  // Typesets the page, then waits for KaTeX's fonts (a page printed before they load shows
  // other fonts). They start loading only once laid out, hence the layout first. Resolves
  // `true`.
  async function run() {
    render(null);
    void document.body.offsetHeight;
    await document.fonts.ready;
    return true;
  }

  // Export as HTML: KaTeX's style sheet as a <style> in the head (the file has no script).
  function finish() {
    if (!formulas().length || document.getElementById('mm-katex')) return;
    const style = document.createElement('style');
    style.id = 'mm-katex';
    style.textContent = css;
    document.head.append(style);
  }

  window.mmMath = { take, render, run, finish };
})();
