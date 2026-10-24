#!/usr/bin/env bash
# Rebuilds syntaxes.packdump: syntect's default syntaxes plus the .sublime-syntax files here,
# compiled once so the app loads them like syntect's own set (no YAML parsing at run time,
# no extra crate in the app). Run inside the container: ./dev.sh resources/syntaxes/build-dump.sh
# The helper crate lives in a temp dir; it checks that every default syntax highlights a few
# samples exactly as syntect's own set does, then writes the dump. See README.md.
set -euo pipefail
cd "$(dirname "$0")"
HERE=$PWD
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/src"
cat > "$TMP/Cargo.toml" <<'TOML'
[package]
name = "syntax-dump"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
# Same version as the app (Cargo.lock): the dump format is syntect's.
syntect = { version = "=5.3.0", default-features = false, features = ["default-syntaxes", "html", "regex-fancy", "yaml-load", "dump-create"] }
TOML
cat > "$TMP/src/main.rs" <<'RS'
use std::time::Instant;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxDefinition, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

const SAMPLES: &[&str] = &[
    "fn main() { let s = \"str\"; println!(\"{s}\"); } // comment\n",
    "def f(x):\n    \"\"\"Doc.\"\"\"\n    return [n ** 2 for n in range(x) if n % 2]\n",
    "const f = (a, b) => a.map(x => x + b).filter(Boolean); /* c */\n<div class=\"x\">&amp;</div>\n",
    "#include <stdio.h>\nint main(void) { printf(\"%d\\n\", 42); return 0; }\n",
    "SELECT a, b FROM t WHERE c = 'x' -- note\n$ echo \"$HOME\" | grep -v '#'\n",
    "key: value\nlist:\n  - 1\n  - \"two\"\n{\"a\": [1, 2.5, true, null]}\n",
];

fn html(set: &SyntaxSet, syntax: &SyntaxReference, code: &str) -> String {
    let style = ClassStyle::SpacedPrefixed { prefix: "hl-" };
    let mut g = ClassedHTMLGenerator::new_with_class_style(syntax, set, style);
    for line in LinesWithEndings::from(code) {
        g.parse_html_for_line_which_includes_newline(line).expect("highlight");
    }
    g.finalize()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, files) = args.split_first().expect("usage: syntax-dump <out> <file.sublime-syntax>...");
    let defaults = SyntaxSet::load_defaults_newlines();
    let mut builder = defaults.clone().into_builder();
    for file in files {
        let text = std::fs::read_to_string(file).expect("read syntax");
        builder.add(SyntaxDefinition::load_from_str(&text, true, None).expect("parse syntax"));
    }
    let set = builder.build();
    for old in defaults.syntaxes() {
        let new = set.find_syntax_by_name(&old.name).expect("default syntax kept");
        for code in SAMPLES {
            assert_eq!(html(&defaults, old, code), html(&set, new, code), "{} changed", old.name);
        }
    }
    let ts = "interface A<T> { x: readonly T[] }\nexport const f = async (a: A<number>): Promise<void> => { await g(a satisfies A<number>); };\n";
    for token in ["ts", "typescript", "tsx"] {
        let syntax = set.find_syntax_by_token(token).expect("TypeScript syntax");
        assert!(html(&set, syntax, ts).contains("hl-storage"), "{token} not highlighted");
    }
    syntect::dumps::dump_to_file(&set, out).expect("write dump");
    let bytes = std::fs::read(out).expect("read dump");
    let start = Instant::now();
    let loaded: SyntaxSet = syntect::dumps::from_binary(&bytes);
    let took = start.elapsed();
    let start = Instant::now();
    let _ = SyntaxSet::load_defaults_newlines();
    println!(
        "{} syntaxes ({} before), {} KB; load {:.1} ms (syntect's own set: {:.1} ms)",
        loaded.syntaxes().len(),
        defaults.syntaxes().len(),
        bytes.len() / 1024,
        took.as_secs_f64() * 1000.0,
        start.elapsed().as_secs_f64() * 1000.0
    );
}
RS
cd "$TMP"
CARGO_TARGET_DIR="$TMP/target" cargo run -q --release -- "$HERE/syntaxes.packdump" "$HERE"/*.sublime-syntax
