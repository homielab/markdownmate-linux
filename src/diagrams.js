// Draws the page's Mermaid diagrams: `render.rs` wraps each ```mermaid block in
// <div class="mermaid"><pre><code>source</code></pre></div>. Runs in the app's script world
// (never the page's), right after the bundled Mermaid, once per page; `diagrams.rs` drives it.
// A drawn diagram is added next to its (then hidden) code; a diagram Mermaid rejects keeps
// its code shown, with a one-line error under it. Results are cached by theme + source, so a
// re-render while typing only draws the block whose source changed.
(function () {
  'use strict';
  if (window.mmDiagrams) return;
  const done = new Map(); // key → {svg} | {error}
  const pending = new Map(); // key → Promise of the same
  const shown = new WeakMap(); // div.mermaid → key of what it shows
  let theme = null;
  let count = 0;
  let generation = 0;

  const pageTheme = () => (document.documentElement.classList.contains('dark') ? 'dark' : 'default');
  const blocks = () => Array.from(document.querySelectorAll('div.mermaid'));
  const keyOf = (div, t) => {
    const pre = div.querySelector(':scope > pre');
    return t + '\n' + (pre ? pre.textContent : '');
  };

  function shortError(error) {
    const lines = String((error && error.message) || error || '')
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean);
    let text = lines[0] || 'not a diagram';
    // "Parse error on line 2:" + the source line + "---^" + "Expecting 'A', 'B', …, got 'C'"
    const got = /got '([^']*)'/.exec(lines[lines.length - 1]);
    if (/^Parse error/.test(text) && got) text += ' unexpected ' + (got[1] === 'EOF' ? 'end' : got[1]);
    return text.length > 200 ? text.slice(0, 199) + '…' : text;
  }

  function clear(div) {
    for (const old of div.querySelectorAll(':scope > .diagram, :scope > .mermaid-error')) old.remove();
  }

  function show(div, result) {
    clear(div);
    if (result.svg !== undefined) {
      const box = document.createElement('div');
      box.className = 'diagram';
      box.innerHTML = result.svg;
      div.append(box);
      div.classList.add('drawn');
    } else {
      const line = document.createElement('p');
      line.className = 'mermaid-error';
      line.textContent = 'Diagram error: ' + result.error;
      div.append(line);
      div.classList.remove('drawn');
    }
  }

  function draw(key) {
    let promise = pending.get(key);
    if (promise) return promise;
    const id = 'mm-diagram-' + ++count;
    const source = key.slice(key.indexOf('\n') + 1);
    promise = mermaid
      .render(id, source)
      .then((r) => ({ svg: r.svg }), (e) => ({ error: shortError(e) }))
      .then((result) => {
        const scratch = document.getElementById('d' + id); // left behind on errors
        if (scratch) scratch.remove();
        pending.delete(key);
        done.set(key, result);
        return result;
      });
    pending.set(key, promise);
    return promise;
  }

  // Right after the page's body was replaced (no wait, so nothing flickers): every block
  // whose diagram is known shows it at once; the others keep showing the diagram that was
  // at the same place before (`previous`, in page order) until `run` has drawn theirs.
  function apply(previous) {
    blocks().forEach((div, i) => {
      const key = keyOf(div, theme);
      const result = done.get(key);
      if (result) {
        show(div, result);
        shown.set(div, key);
      } else if (previous && previous[i]) {
        div.append(previous[i]);
        div.classList.add('drawn');
      }
    });
  }

  // The diagrams of the old body, before it is replaced (see `apply`).
  function take() {
    return blocks().map((div) => div.querySelector(':scope > .diagram'));
  }

  // Draws every block not showing its own diagram yet, one at a time. Resolves `true` when
  // the page is complete, `false` if a newer run took over (what was drawn stays cached).
  async function run() {
    const mine = ++generation;
    const t = pageTheme();
    if (t !== theme) {
      await Promise.all(pending.values());
      if (mine !== generation) return false;
      theme = t;
      mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', theme, suppressErrorRendering: true });
    }
    for (const div of blocks()) {
      const key = keyOf(div, theme);
      if (shown.get(div) === key) continue;
      const result = done.get(key) || (await draw(key));
      if (mine !== generation) return false;
      if (div.isConnected) {
        show(div, result);
        shown.set(div, key);
      }
    }
    if (done.size > 64) {
      const keep = new Set(blocks().map((div) => keyOf(div, theme)));
      for (const key of done.keys()) if (!keep.has(key)) done.delete(key);
    }
    return true;
  }

  // Export as HTML: a static page, the code of drawn diagrams removed.
  function finish() {
    for (const div of blocks()) {
      if (div.classList.contains('drawn')) {
        const pre = div.querySelector(':scope > pre');
        if (pre) pre.remove();
      }
    }
    return '<!doctype html>' + document.documentElement.outerHTML;
  }

  window.mmDiagrams = { apply, take, run, finish };
})();
