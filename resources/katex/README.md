# KaTeX

[KaTeX](https://katex.org) 0.18.9 from the npm package `katex`, MIT licence
(`LICENSE`):

- `katex.min.js` is the unmodified `dist/katex.min.js`.
- `katex.min.css` is `dist/katex.min.css` with its fonts embedded: each
  `@font-face` keeps only its woff2 file, as a `data:` URL (`inline-fonts.py`
  makes it), so the preview and exported HTML need no font files and load
  nothing from the network. Upstream ships the fonts under the same MIT
  licence; their glyphs are adapted from the MathJax fonts, which upstream marks
  as SIL Open Font License 1.1 (`OFL.txt`).

The app embeds both (`src/math.rs`) and runs them only on pages that have a
formula (`$…$`, `$$…$$` or a ```` ```math ```` block).

To update: take the files from a newer release
(`npm pack katex@<version>` and unpack it), then

    cp package/dist/katex.min.js resources/katex/
    python3 resources/katex/inline-fonts.py package/dist > resources/katex/katex.min.css

update the version here, in `src/math.rs`'s test and in `debian/copyright`, and
check a few formulas (inline, display, a matrix, an invalid one) in Preview,
Print and Export as HTML / PDF.
