//! How the indexer cuts a file's text into passages, and a PDF's running
//! headers and footers out of its pages. Lengths are in characters.

use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use regex::Regex;

use crate::text::{PyTrim, fold_spaces, is_py_space, splitlines};

/// About 250 tokens: a section of a note stays its own passage instead of an
/// average of its neighbours. Well under the model's 2048 tokens.
pub const PASSAGE_CHARS: usize = 900;
const OVERLAP_CHARS: usize = 150;
/// Joins the model in meta: a change to how passages are cut redoes the index.
pub const PASSAGES_VERSION: u32 = 3;

/// The last start of `needle` lying wholly in chars[from..to], as Python's rfind.
fn rfind(chars: &[char], needle: &[char], from: usize, to: usize) -> Option<usize> {
    let to = to.min(chars.len());
    if to < from + needle.len() {
        return None;
    }
    (from..=to - needle.len())
        .rev()
        .find(|&i| chars[i..i + needle.len()] == *needle)
}

/// The first start of `needle` lying wholly in chars[from..to], as Python's find.
fn find(chars: &[char], needle: char, from: usize, to: usize) -> Option<usize> {
    (from..to.min(chars.len())).find(|&i| chars[i] == needle)
}

/// A paragraph of folded text cut into pieces of about PASSAGE_CHARS,
/// overlapping, at a sentence or else a word.
fn split_long(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = start + size;
        if end >= chars.len() {
            out.push(chars[start..].iter().collect::<String>());
            break;
        }
        let cut = rfind(&chars, &['.', ' '], start + size / 2, end)
            .or_else(|| rfind(&chars, &[' '], start + size / 2, end))
            .map_or(end, |c| c + 1);
        out.push(
            chars[start..cut]
                .iter()
                .collect::<String>()
                .py_trim()
                .to_string(),
        );
        let back = cut.saturating_sub(OVERLAP_CHARS);
        start = find(&chars, ' ', back, cut).map_or(back, |s| s + 1);
    }
    out.into_iter().filter(|p| !p.is_empty()).collect()
}

type Section = (Vec<String>, String);

/// [(headings above it, its text)] of a text: a Markdown heading line opens a
/// section, outside code fences, whose "# comment" lines are not headings.
fn sections(text: &str) -> Vec<Section> {
    static HEADING: OnceLock<Regex> = OnceLock::new();
    let heading_re = HEADING
        .get_or_init(|| Regex::new(r"^(#{1,6})[\s\x1c-\x1f]+(.+?)[\s\x1c-\x1f#]*$").unwrap());
    let mut out: Vec<(Vec<(usize, String)>, String)> = Vec::new();
    let mut trail: Vec<(usize, String)> = Vec::new();
    let mut lines: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in splitlines(text) {
        let stripped = line.trim_start_matches(is_py_space);
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            fenced = !fenced;
        }
        match heading_re.captures(line).filter(|_| !fenced) {
            Some(h) => {
                out.push((trail.clone(), lines.join("\n")));
                let level = h[1].len();
                trail.retain(|(l, _)| *l < level);
                trail.push((level, h[2].to_string()));
                lines.clear();
            }
            None => lines.push(line),
        }
    }
    out.push((trail, lines.join("\n")));
    out.into_iter()
        .filter(|(_, body)| !body.py_trim().is_empty())
        .map(|(t, body)| (t.into_iter().map(|(_, h)| h).collect(), body))
        .collect()
}

/// The text of sections, one folded paragraph a line, with each heading
/// below depth, the first time it appears, as a line of its own.
fn paragraphs(sections: &[Section], depth: usize) -> Vec<String> {
    static BLANK: OnceLock<Regex> = OnceLock::new();
    let blank = BLANK.get_or_init(|| Regex::new(r"\n[\s\x1c-\x1f]*\n").unwrap());
    let mut lines = Vec::new();
    let mut shown: &[String] = &[];
    for (trail, body) in sections {
        for n in depth..trail.len() {
            if shown.get(..n + 1) != Some(&trail[..n + 1]) {
                lines.push(trail[n].clone());
            }
        }
        shown = trail;
        lines.extend(
            blank
                .split(body)
                .filter(|p| !p.py_trim().is_empty())
                .map(fold_spaces),
        );
    }
    lines
}

/// Text cut into passages of up to about PASSAGE_CHARS, each starting with a
/// line "<title> > <heading> > <subheading>". A file or a section that fits
/// stays whole, so a short note keeps its context; a longer one is cut at its
/// subheadings, and a section without any at its paragraphs.
pub fn passages(text: &str, title: &str) -> Vec<String> {
    passages_sized(text, title, PASSAGE_CHARS)
}

fn passages_sized(text: &str, title: &str, size: usize) -> Vec<String> {
    let mut out = Vec::new();
    cut(&mut out, title, size, &[], &sections(text));
    out
}

fn cut(out: &mut Vec<String>, title: &str, size: usize, trail: &[String], group: &[Section]) {
    let depth = trail.len();
    let head = std::iter::once(title)
        .chain(trail.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" > ");
    let lines = paragraphs(group, depth);
    let deeper = group.iter().any(|s| s.0.len() > depth);
    if lines.join("\n").chars().count() <= size || !deeper {
        let mut chunk = String::new();
        let mut chunk_len = 0;
        for piece in lines.iter().flat_map(|l| split_long(l, size)) {
            let piece_len = piece.chars().count();
            if chunk_len > 0 && chunk_len + 1 + piece_len > size {
                out.push(format!("{head}\n{chunk}"));
                chunk.clear();
                chunk_len = 0;
            }
            if chunk_len > 0 {
                chunk.push('\n');
                chunk_len += 1;
            }
            chunk.push_str(&piece);
            chunk_len += piece_len;
        }
        if chunk_len > 0 {
            out.push(format!("{head}\n{chunk}"));
        }
        return;
    }
    // Text before the first subheading, then each subheading with what it holds.
    let mut runs: Vec<(Vec<String>, Vec<Section>)> = Vec::new();
    for section in group {
        let key: Vec<String> = section.0.iter().take(depth + 1).cloned().collect();
        match runs.last_mut() {
            Some((k, run)) if *k == key => run.push(section.clone()),
            _ => runs.push((key, vec![section.clone()])),
        }
    }
    for (key, run) in runs {
        cut(out, title, size, &key, &run);
    }
}

/// The forms a line is compared in across pages: itself, case and spacing
/// aside, and with each number of up to 4 digits given as its distance to
/// the page's number, so "Page 3 of 12" on page 3 matches "Page 4 of 12" on
/// page 4, but ticket 207952278 never matches ticket 207952279.
fn running_keys(line: &str, page: usize) -> HashSet<String> {
    static PAGE_NUMBER: OnceLock<Regex> = OnceLock::new();
    let re = PAGE_NUMBER.get_or_init(|| Regex::new(r"\b[0-9]{1,4}\b").unwrap());
    let text = fold_spaces(line).to_lowercase();
    let mut keys = HashSet::from([text.clone()]);
    for m in re.find_iter(&text) {
        let n: i64 = m.as_str().parse().unwrap_or(0);
        keys.insert(format!(
            "{}#{}{}",
            &text[..m.start()],
            n - page as i64,
            &text[m.end()..]
        ));
    }
    keys
}

/// A PDF's page texts without their running headers and footers: a line
/// among the first or last three of a page that more than half the pages with
/// text share is kept only where it first appears.
pub fn without_running_lines(pages: Vec<String>) -> Vec<String> {
    let lines: Vec<Vec<&str>> = pages.iter().map(|t| splitlines(t)).collect();
    let edges: Vec<HashSet<usize>> = lines
        .iter()
        .map(|page| {
            let filled: Vec<usize> = (0..page.len())
                .filter(|&i| !page[i].py_trim().is_empty())
                .collect();
            let tail = filled.len().saturating_sub(3);
            filled
                .iter()
                .take(3)
                .chain(filled[tail..].iter())
                .copied()
                .collect()
        })
        .collect();
    let with_text = edges.iter().filter(|e| !e.is_empty()).count();
    if with_text < 3 {
        return pages;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (n, (page, e)) in lines.iter().zip(&edges).enumerate() {
        let keys: HashSet<String> = e.iter().flat_map(|&i| running_keys(page[i], n)).collect();
        for key in keys {
            *counts.entry(key).or_default() += 1;
        }
    }
    let running: HashSet<String> = counts
        .into_iter()
        .filter(|(_, c)| c * 2 > with_text)
        .map(|(k, _)| k)
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (n, (page, e)) in lines.iter().zip(&edges).enumerate() {
        let mut kept = Vec::new();
        for (i, line) in page.iter().enumerate() {
            let keys: HashSet<String> = if e.contains(&i) {
                running_keys(line, n)
                    .intersection(&running)
                    .cloned()
                    .collect()
            } else {
                HashSet::new()
            };
            if keys.is_disjoint(&seen) {
                kept.push(*line);
            }
            seen.extend(keys);
        }
        out.push(kept.join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "intro\n\n## lessive\n- savon\n- soude\n\n## version laine :\n100g savon\n\n4L eau\n\
                        # Shell\n```sh\n# not a heading\n```\n### Sub\ntext";

    #[test]
    fn a_short_note_stays_one_passage() {
        assert_eq!(
            passages(NOTE, "Lessive.md"),
            vec![
                "Lessive.md\nintro\nlessive\n- savon - soude\nversion laine :\n100g savon\n4L eau\nShell\n\
                 ```sh # not a heading ```\nSub\ntext"
            ]
        );
    }

    #[test]
    fn a_longer_one_is_cut_at_its_headings() {
        assert_eq!(
            passages_sized(NOTE, "Lessive.md", 40),
            vec![
                "Lessive.md\nintro",
                "Lessive.md > lessive\n- savon - soude",
                "Lessive.md > version laine :\n100g savon\n4L eau",
                "Lessive.md > Shell\n```sh # not a heading ```\nSub\ntext",
            ]
        );
    }

    #[test]
    fn a_long_paragraph_is_cut_near_the_size() {
        let text = (0..300)
            .map(|n| format!("Phrase {n} sur le jardin."))
            .collect::<Vec<_>>()
            .join(" ");
        let cut = passages(&text, "long.md");
        assert!(cut.len() > 5 && cut.iter().all(|p| p.chars().count() <= PASSAGE_CHARS + 20));
    }

    #[test]
    fn running_lines_kept_once() {
        let pages: Vec<String> = ["ortie", "sureau", "tilleul", "verveine"]
            .iter()
            .enumerate()
            .map(|(i, w)| {
                format!(
                    "Rapport annuel CONFIDENTIEL\nCotisation {w}\nau bureau {}0\nPage {} sur 4",
                    i + 1,
                    i + 1
                )
            })
            .collect();
        let out = without_running_lines(pages);
        assert_eq!(out.iter().filter(|t| t.contains("CONFIDENTIEL")).count(), 1);
        assert_eq!(out.iter().filter(|t| t.contains("sur 4")).count(), 1);
        assert!(
            out.iter()
                .enumerate()
                .all(|(i, t)| t.contains(&format!("bureau {}0", i + 1)))
        );
    }
}
