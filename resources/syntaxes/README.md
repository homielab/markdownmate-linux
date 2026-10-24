# Extra code highlighting syntaxes

syntect's default syntaxes have no TypeScript. These two add it for ```` ```ts ````,
```` ```typescript ```` (and `.mts` / `.cts`) and ```` ```tsx ```` code blocks:

- `TypeScript.sublime-syntax` — from [bat](https://github.com/sharkdp/bat)
  (`assets/syntaxes/02_Extra/TypeScript.sublime-syntax` at commit `d7b6519`), converted from
  `TypeScript.tmLanguage` of Microsoft's
  [TypeScript-Sublime-Plugin](https://github.com/microsoft/TypeScript-Sublime-Plugin) at
  commit `ba45efd`. Unmodified.
- `TypeScriptReact.sublime-syntax` — from [Zola](https://github.com/getzola/zola) 0.19.2
  (`sublime/syntaxes/extra/TypeScriptReact.sublime-syntax`), converted from
  `TypeScriptReact.tmLanguage` of the same plugin at commit `a607ddf`. Unmodified.

Both grammars are © Microsoft Corporation under the Apache License 2.0 (`LICENSE`, the
plugin's licence file).

The app doesn't parse them at run time: `build-dump.sh` compiles syntect's default set plus
every `*.sublime-syntax` here into `syntaxes.packdump`, which `src/render.rs` embeds and loads
like syntect's own set (≈ 4 ms, on the first highlighted code block). The script builds a
helper crate in a temp dir (syntect with `yaml-load`, so the app itself needs no YAML
parser), checks that every default syntax still highlights its samples exactly as before and
that `ts` / `typescript` / `tsx` are highlighted, then writes the dump (the same bytes on every
run). After changing a syntax here, or syntect's version in `Cargo.toml` (also pinned in the
script), run it again and commit the new dump:

    ./dev.sh resources/syntaxes/build-dump.sh

To update a syntax, take the newer file from the same place, update the commits above and
check a few TypeScript / TSX blocks in Preview.
