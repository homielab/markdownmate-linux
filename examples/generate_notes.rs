//! Measurement helper (not part of the app): writes a folder of generated notes for
//! `scripts/measure-folder.sh` (docs/PERFORMANCE.md). The same folder on every run.
//! `cargo run --release --example generate_notes -- /tmp/notes 10000`
//!
//! Notes are spread over 20 areas × 10 topics (`Area 3/Topic 7/Note 1234.md`, about 50 per
//! folder at 10,000). Each has a heading, 1–3 tags out of 170 plus a nested `#areaN/topicM`
//! tag, 2–6 sections of sentences, lists, tasks and quotes, a fenced code block (with a `#tag`
//! and a `[[link]]` inside that must be ignored), a wiki link to a random note per section
//! and, one time in three, a relative Markdown link to a note of the same folder.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const AREAS: usize = 20;
const TOPICS: usize = 10;
const TAGS: usize = 170;
const WORDS: &[&str] = &[
    "note", "idea", "plan", "draft", "meeting", "project", "review", "summary", "question",
    "answer", "design", "release", "budget", "travel", "reading", "garden", "recipe", "client",
    "server", "follow", "update", "simple", "quick", "local", "folder", "markdown", "write",
    "later", "today", "week", "list", "check", "open", "close", "small", "large", "first",
    "second", "again", "every",
];

/// A fixed-seed generator (xorshift), so every run writes the same folder.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }

    fn sentence(&mut self) -> String {
        let n = 6 + self.below(10);
        let mut s: Vec<&str> = (0..n).map(|_| WORDS[self.below(WORDS.len())]).collect();
        let first = s[0][..1].to_uppercase() + &s[0][1..];
        s[0] = &first;
        format!("{}.", s.join(" "))
    }
}

/// Folder of note `i` (relative) and its file name without `.md`.
fn place(i: usize) -> (String, String) {
    let area = i % AREAS;
    let topic = (i / AREAS) % TOPICS;
    (format!("Area {area}/Topic {topic}"), format!("Note {i}"))
}

fn note(i: usize, count: usize, rng: &mut Rng) -> String {
    let (dir, name) = place(i);
    let mut md = format!("# {name}\n\n");
    let tags: Vec<String> = (0..1 + rng.below(3))
        .map(|_| format!("#tag{}", rng.below(TAGS)))
        .collect();
    let (area, topic) = (i % AREAS, (i / AREAS) % TOPICS);
    let _ = writeln!(md, "{} #area{area}/topic{topic}\n", tags.join(" "));
    for s in 0..2 + rng.below(5) {
        let _ = writeln!(md, "## Section {s}\n");
        let other = place(rng.below(count)).1;
        let _ = writeln!(
            md,
            "{} See [[{other}]] for more. {}\n",
            rng.sentence(),
            rng.sentence()
        );
        match rng.below(4) {
            0 => {
                let _ = writeln!(
                    md,
                    "- {}\n- {}\n  - {}\n",
                    rng.sentence(),
                    rng.sentence(),
                    rng.sentence()
                );
            }
            1 => {
                let _ = writeln!(md, "- [ ] {}\n- [x] {}\n", rng.sentence(), rng.sentence());
            }
            2 => {
                let _ = writeln!(md, "> {}\n", rng.sentence());
            }
            _ => {
                let _ = writeln!(md, "{}\n", rng.sentence());
            }
        }
    }
    let _ = writeln!(
        md,
        "```sh\n# not a heading #notatag [[Not A Link]]\necho {i}\n```\n"
    );
    if rng.below(3) == 0 {
        // A note of the same folder: same area and topic, another number.
        let j = (i + AREAS * TOPICS) % count;
        if place(j).0 == dir && j != i {
            let _ = writeln!(md, "Next: [Note {j}](Note%20{j}.md)\n");
        }
    }
    md
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(root), count) = (args.next(), args.next()) else {
        eprintln!("usage: generate_notes <folder> [count, default 10000]");
        std::process::exit(2);
    };
    let count: usize = count.and_then(|c| c.parse().ok()).unwrap_or(10_000).max(1);
    let root = Path::new(&root);
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut bytes = 0;
    for i in 0..count {
        let (dir, name) = place(i);
        let dir = root.join(dir);
        fs::create_dir_all(&dir)?;
        let text = note(i, count, &mut rng);
        bytes += text.len();
        fs::write(dir.join(format!("{name}.md")), text)?;
    }
    println!(
        "{count} notes, {:.1} MB, in {}",
        bytes as f64 / 1e6,
        root.display()
    );
    Ok(())
}
