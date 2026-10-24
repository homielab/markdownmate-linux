# Mermaid

`mermaid.min.js` is [Mermaid](https://mermaid.js.org) 11.17.2, the unmodified
`dist/mermaid.min.js` of the npm package `mermaid`, MIT licence (`LICENSE`; the
libraries it bundles keep their notices inside the file). The app embeds it
(`src/diagrams.rs`) and runs it only on pages that have a ```` ```mermaid ```` block.

To update: replace the file with the same file from a newer release
(`npm pack mermaid@11` → `package/dist/mermaid.min.js`), update the version here
and in `debian/copyright`, and check a few diagrams in Preview, Print and
Export as HTML.
