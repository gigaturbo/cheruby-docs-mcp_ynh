//! The text inside a document, for the tool server and the indexer.
//!
//! `document_text` reads Word, OpenDocument, PowerPoint, Excel, EPUB and email
//! files: one paragraph, row or header a line, headings as Markdown "#" lines,
//! which the indexer cuts passages at. Each format is read from its XML.
//! `html_text` reads HTML and XML for the index, `pdf_pages` a PDF's pages
//! with PDFium. Errors are messages a user should see.

use std::{
    collections::HashMap,
    io::{Read, Seek},
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use pdfium_render::prelude::*;
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use regex::Regex;

/// What a document may unpack to in all, so a small file cannot fill the memory.
const MAX_XML_BYTES: u64 = 20_000_000;

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const SLIDE: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const RELATION: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const ODF_TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const ODF_TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const ODF_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const ODF_SHOW: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";
const EPUB_CONTAINER: &str = "urn:oasis:names:tc:opendocument:xmlns:container";
const OPF: &str = "http://www.idpf.org/2007/opf";

pub const DOCUMENT_SUFFIXES: &[&str] = &[
    ".docx", ".odt", ".pptx", ".odp", ".xlsx", ".ods", ".epub", ".eml",
];

/// Lines as Python's str.splitlines() cuts them.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        out.push(&text[start..i]);
        start = i + c.len_utf8();
        if c == '\r' && chars.peek().map(|&(_, n)| n) == Some('\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Whether Python's str.isspace() says so: Unicode's spaces, and the
/// separators \x1c to \x1f, which PDFium gives for some broken ligatures.
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// The words of a text, as Python's str.split() cuts them.
pub fn py_split(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_py_space).filter(|w| !w.is_empty())
}

/// Python's str.strip().
pub trait PyTrim {
    fn py_trim(&self) -> &str;
}

impl PyTrim for str {
    fn py_trim(&self) -> &str {
        self.trim_matches(is_py_space)
    }
}

/// Text folded as Python's " ".join(text.split()).
pub fn fold_spaces(text: &str) -> String {
    py_split(text).collect::<Vec<_>>().join(" ")
}

/// An XML element: its name and attributes as "{namespace}local", its
/// children in order, text included.
#[derive(Debug, Default)]
pub struct Element {
    pub tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

#[derive(Debug)]
enum Node {
    Element(Element),
    Text(String),
}

fn qualified(ns: &ResolveResult, local: &str) -> String {
    match ns {
        ResolveResult::Bound(n) => format!("{{{}}}{local}", n.as_ref()),
        _ => local.to_string(),
    }
}

/// The document's root element; an error names what made it unreadable.
pub fn parse_xml(data: &[u8]) -> Result<Element, String> {
    let text = std::str::from_utf8(data).map_err(|e| e.to_string())?;
    let mut reader = NsReader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    let bad = |e: &dyn std::fmt::Display| e.to_string();
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(|e| bad(&e))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let mut el = Element {
                    tag: qualified(&ns, e.local_name().as_ref()),
                    ..Default::default()
                };
                for a in e.attributes() {
                    let a = a.map_err(|e| bad(&e))?;
                    let (ans, local) = reader.resolver().resolve_attribute(a.key);
                    if a.key.as_ref().starts_with("xmlns") {
                        continue;
                    }
                    let value = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|e| bad(&e))?;
                    el.attrs
                        .push((qualified(&ans, local.as_ref()), value.into_owned()));
                }
                if empty {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Node::Element(el)),
                        None => root = Some(el),
                    }
                } else {
                    stack.push(el);
                }
            }
            Event::End(_) => {
                let el = stack.pop().ok_or("unbalanced XML")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                if let Some(parent) = stack.last_mut() {
                    parent
                        .children
                        .push(Node::Text(t.html_content().into_owned()));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(parent) = stack.last_mut() {
                    let name = r.html_content();
                    let text = match r.resolve_char_ref().map_err(|e| bad(&e))? {
                        Some(c) => c.to_string(),
                        None => match name.as_ref() {
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "amp" => "&".into(),
                            "apos" => "'".into(),
                            "quot" => "\"".into(),
                            other => return Err(format!("undefined entity &{other};")),
                        },
                    };
                    parent.children.push(Node::Text(text));
                }
            }
            Event::CData(t) => {
                if let Some(parent) = stack.last_mut() {
                    parent
                        .children
                        .push(Node::Text(t.html_content().into_owned()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("no element found".into());
    }
    root.ok_or_else(|| "no element found".into())
}

impl Element {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn local(&self) -> &str {
        self.tag.rsplit('}').next().unwrap_or(&self.tag)
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// This element and every one below it, in document order.
    fn iter(&self) -> Vec<&Element> {
        let mut out = vec![self];
        for child in self.elements() {
            out.extend(child.iter());
        }
        out
    }

    /// iter() of the elements with this tag.
    fn iter_tag(&self, tag: &str) -> Vec<&Element> {
        self.iter().into_iter().filter(|e| e.tag == tag).collect()
    }

    /// The first element down this path of child tags.
    fn find(&self, path: &[&str]) -> Option<&Element> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        self.elements()
            .filter(|e| e.tag == *first)
            .find_map(|e| e.find(rest))
    }

    /// The text of the first child with this tag.
    fn findtext(&self, tag: &str) -> Option<String> {
        self.elements().find(|e| e.tag == tag).map(|e| e.text())
    }

    /// All the text below, as ElementTree's itertext() joined.
    fn text(&self) -> String {
        let mut out = String::new();
        for n in &self.children {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => out.push_str(&e.text()),
            }
        }
        out
    }
}

fn ns(namespace: &str, local: &str) -> String {
    format!("{{{namespace}}}{local}")
}

/// The members of a zipped document, read up to MAX_XML_BYTES in all.
struct Package<R: Read + Seek> {
    zip: zip::ZipArchive<R>,
    left: u64,
}

impl<R: Read + Seek> Package<R> {
    fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let missing = || {
            format!(
                "the document has no {}; it may be damaged.",
                if name.is_empty() { "main part" } else { name }
            )
        };
        let mut file = self.zip.by_name(name).map_err(|_| missing())?;
        let size = file.size();
        if size > self.left {
            return Err("the document's text is too large to read.".into());
        }
        self.left -= size;
        let mut data = Vec::with_capacity(size as usize);
        file.by_ref()
            .take(size)
            .read_to_end(&mut data)
            .map_err(|e| format!("the document is damaged: {e}"))?;
        Ok(data)
    }

    fn xml(&mut self, name: &str) -> Result<Element, String> {
        parse_xml(&self.read(name)?).map_err(|e| format!("the document is damaged: {e}"))
    }

    /// {relationship id: member} of an Office Open XML part.
    fn parts(&mut self, part: &str) -> Result<HashMap<String, String>, String> {
        let (folder, name) = part.rsplit_once('/').unwrap_or(("", part));
        let rels = self.xml(&posix_join(folder, &format!("_rels/{name}.rels")))?;
        Ok(rels
            .elements()
            .map(|r| {
                (
                    r.get("Id").unwrap_or("").to_string(),
                    member(folder, r.get("Target").unwrap_or("")),
                )
            })
            .collect())
    }
}

fn posix_join(folder: &str, path: &str) -> String {
    if folder.is_empty() {
        path.to_string()
    } else {
        format!("{folder}/{path}")
    }
}

/// A path with "." and ".." folded, as posixpath.normpath.
fn normpath(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !path.starts_with('/') {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    match (path.starts_with('/'), joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// The zip member a link from a part in folder names.
fn member(folder: &str, href: &str) -> String {
    let raw = href.split('#').next().unwrap_or("");
    let path = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    match path.strip_prefix('/') {
        Some(p) => normpath(p),
        None => normpath(&posix_join(folder, &path)),
    }
}

/// 1 to 6 for a heading paragraph of a docx or odt, else 0.
fn heading_level(el: &Element) -> usize {
    if el.tag == ns(ODF_TEXT, "h") {
        let level = el.get(&ns(ODF_TEXT, "outline-level")).unwrap_or("1");
        return match level.parse::<usize>() {
            Ok(n) if level.chars().all(|c| c.is_ascii_digit()) => n.clamp(1, 6),
            _ => 1,
        };
    }
    static HEADING_STYLE: OnceLock<Regex> = OnceLock::new();
    // Word names its heading styles in the document's language: Heading1, Titre1.
    let style_re =
        HEADING_STYLE.get_or_init(|| Regex::new(r"(?i)^(?:heading|titre)\s*([1-6])$").unwrap());
    let style = el
        .find(&[&ns(WORD, "pPr"), &ns(WORD, "pStyle")])
        .and_then(|s| s.get(&ns(WORD, "val")));
    style
        .and_then(|s| style_re.captures(s))
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(0)
}

/// The paragraphs of a docx or odt part, one a line.
fn paragraphs(root: &Element) -> String {
    let mut lines = Vec::new();
    for el in root.iter() {
        if matches!(el.local(), "p" | "h") {
            let text = el.text();
            let text = text.py_trim();
            if !text.is_empty() {
                let level = heading_level(el);
                lines.push(if level > 0 {
                    format!("{} {text}", "#".repeat(level))
                } else {
                    text.to_string()
                });
            }
        }
    }
    lines.join("\n")
}

/// Lines of slide n from its (is title, text) pairs: "# Slide <n>: <title>", then the rest.
fn slide(n: usize, texts: Vec<(bool, String)>) -> Vec<String> {
    let texts: Vec<(bool, String)> = texts
        .into_iter()
        .map(|(t, s)| (t, s.py_trim().to_string()))
        .filter(|(_, s)| !s.is_empty())
        .collect();
    let title = texts
        .iter()
        .filter(|(t, _)| *t)
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut lines = vec![if title.is_empty() {
        format!("# Slide {n}")
    } else {
        format!("# Slide {n}: {title}")
    }];
    lines.extend(texts.into_iter().filter(|(t, _)| !t).map(|(_, s)| s));
    lines
}

/// A pptx's slides in the order shown, speaker notes left out.
fn pptx_text<R: Read + Seek>(pkg: &mut Package<R>) -> Result<String, String> {
    let deck = "ppt/presentation.xml";
    let parts = pkg.parts(deck)?;
    let mut lines = Vec::new();
    let ids: Vec<String> = pkg
        .xml(deck)?
        .iter_tag(&ns(SLIDE, "sldId"))
        .iter()
        .map(|r| r.get(&ns(RELATION, "id")).unwrap_or("").to_string())
        .collect();
    for (n, id) in ids.iter().enumerate() {
        let root = pkg.xml(parts.get(id).map(String::as_str).unwrap_or(""))?;
        let mut titles: Vec<*const Element> = Vec::new();
        for shape in root.iter_tag(&ns(SLIDE, "sp")) {
            let ph = shape.find(&[&ns(SLIDE, "nvSpPr"), &ns(SLIDE, "nvPr"), &ns(SLIDE, "ph")]);
            if ph.is_some_and(|ph| matches!(ph.get("type"), Some("title" | "ctrTitle"))) {
                titles.extend(
                    shape
                        .iter_tag(&ns(DRAWING, "p"))
                        .into_iter()
                        .map(|p| p as *const Element),
                );
            }
        }
        let texts = root
            .iter_tag(&ns(DRAWING, "p"))
            .into_iter()
            .map(|p| (titles.contains(&(p as *const _)), p.text()))
            .collect();
        lines.extend(slide(n + 1, texts));
    }
    Ok(lines.join("\n"))
}

/// An odp's slides in order, speaker notes left out.
fn odp_text<R: Read + Seek>(pkg: &mut Package<R>) -> Result<String, String> {
    let content = pkg.xml("content.xml")?;
    let mut lines = Vec::new();
    for (n, page) in content
        .iter_tag(&ns(ODF_DRAW, "page"))
        .into_iter()
        .enumerate()
    {
        let mut texts = Vec::new();
        for shape in page.elements().filter(|s| s.tag != ns(ODF_SHOW, "notes")) {
            let title = shape.get(&ns(ODF_SHOW, "class")) == Some("title");
            texts.extend(
                shape
                    .iter_tag(&ns(ODF_TEXT, "p"))
                    .into_iter()
                    .map(|p| (title, p.text())),
            );
        }
        lines.extend(slide(n + 1, texts));
    }
    Ok(lines.join("\n"))
}

/// An xlsx's sheets in order, each under "# <name>", a row a line, its cells
/// tab-separated as stored: a date is its serial number, a formula its last result.
fn xlsx_text<R: Read + Seek>(pkg: &mut Package<R>) -> Result<String, String> {
    let book = "xl/workbook.xml";
    let parts = pkg.parts(book)?;
    let strings = parts
        .values()
        .find(|m| m.ends_with("/sharedStrings.xml"))
        .cloned();
    let shared: Vec<String> = match strings {
        None => Vec::new(),
        Some(member) => pkg
            .xml(&member)?
            .iter_tag(&ns(SHEET, "si"))
            .into_iter()
            .map(|si| {
                let direct = si
                    .elements()
                    .filter(|e| e.tag == ns(SHEET, "t"))
                    .map(Element::text);
                let runs = si
                    .elements()
                    .filter(|e| e.tag == ns(SHEET, "r"))
                    .flat_map(|r| {
                        r.elements()
                            .filter(|e| e.tag == ns(SHEET, "t"))
                            .map(Element::text)
                            .collect::<Vec<_>>()
                    });
                direct.chain(runs).collect()
            })
            .collect(),
    };
    let sheets: Vec<(String, String)> = pkg
        .xml(book)?
        .iter_tag(&ns(SHEET, "sheet"))
        .into_iter()
        .map(|s| {
            (
                s.get("name").unwrap_or("").to_string(),
                s.get(&ns(RELATION, "id")).unwrap_or("").to_string(),
            )
        })
        .collect();
    let mut lines = Vec::new();
    for (name, id) in sheets {
        lines.push(format!("# {name}"));
        let sheet = pkg.xml(parts.get(&id).map(String::as_str).unwrap_or(""))?;
        for row in sheet.iter_tag(&ns(SHEET, "row")) {
            let mut cells = Vec::new();
            for c in row.iter_tag(&ns(SHEET, "c")) {
                let mut value = c.findtext(&ns(SHEET, "v")).unwrap_or_default();
                match c.get("t") {
                    Some("s") => {
                        value = match value.parse::<usize>() {
                            Ok(i)
                                if value.chars().all(|c| c.is_ascii_digit())
                                    && i < shared.len() =>
                            {
                                shared[i].clone()
                            }
                            _ => String::new(),
                        }
                    }
                    Some("inlineStr") => {
                        value = c
                            .iter_tag(&ns(SHEET, "t"))
                            .into_iter()
                            .map(Element::text)
                            .collect()
                    }
                    _ => {}
                }
                cells.push(value.py_trim().to_string());
            }
            if cells.iter().any(|c| !c.is_empty()) {
                lines.push(
                    cells
                        .into_iter()
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>()
                        .join("\t"),
                );
            }
        }
    }
    Ok(lines.join("\n"))
}

/// An ods's sheets in order, each under "# <name>", a row a line, its cells
/// tab-separated; a row or cell repeated is given once.
fn ods_text<R: Read + Seek>(pkg: &mut Package<R>) -> Result<String, String> {
    let content = pkg.xml("content.xml")?;
    let mut lines = Vec::new();
    for table in content.iter_tag(&ns(ODF_TABLE, "table")) {
        lines.push(format!(
            "# {}",
            table.get(&ns(ODF_TABLE, "name")).unwrap_or("")
        ));
        for row in table.iter_tag(&ns(ODF_TABLE, "table-row")) {
            let cells: Vec<String> = row
                .elements()
                .map(|cell| {
                    let texts: Vec<String> = cell
                        .iter_tag(&ns(ODF_TEXT, "p"))
                        .into_iter()
                        .map(|p| p.text().py_trim().to_string())
                        .collect();
                    texts.join(" ").py_trim().to_string()
                })
                .collect();
            if cells.iter().any(|c| !c.is_empty()) {
                lines.push(
                    cells
                        .into_iter()
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>()
                        .join("\t"),
                );
            }
        }
    }
    Ok(lines.join("\n"))
}

const BLOCKS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "tr",
    "td",
    "th",
    "dt",
    "dd",
    "blockquote",
    "pre",
    "section",
    "article",
    "header",
    "footer",
    "aside",
    "nav",
    "table",
    "ul",
    "ol",
    "dl",
    "figure",
    "figcaption",
    "hr",
];
const SKIPPED: &[&str] = &["script", "style", "title"];

/// Collects an HTML page's text, a line per block, headings as "#" lines;
/// with xml, every element is a block. Lenient as Python's HTMLParser.
struct HtmlText {
    parts: Vec<String>,
    skip: usize,
    xml: bool,
}

impl HtmlText {
    fn start(&mut self, tag: &str) {
        if SKIPPED.contains(&tag) {
            self.skip += 1;
        } else if let Some(level) = heading(tag) {
            self.parts.push(format!("\n{} ", "#".repeat(level)));
        } else if self.xml || BLOCKS.contains(&tag) {
            self.parts.push("\n".into());
        }
    }

    fn end(&mut self, tag: &str) {
        if SKIPPED.contains(&tag) {
            self.skip = self.skip.saturating_sub(1);
        } else if self.xml || heading(tag).is_some() || BLOCKS.contains(&tag) {
            self.parts.push("\n".into());
        }
    }

    fn data(&mut self, data: &str) {
        if self.skip == 0 {
            self.parts.push(data.to_string());
        }
    }
}

fn heading(tag: &str) -> Option<usize> {
    match tag.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some((d - b'0') as usize),
        _ => None,
    }
}

/// The tag name at the start of s, lower case, and where it ends.
fn tag_name(s: &str) -> Option<(String, usize)> {
    let end = s
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(s.len());
    let name = &s[..end];
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((name.to_lowercase(), end))
}

/// Where the tag that starts at the beginning of s ends, after its ">",
/// skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// The text of an HTML page, a line per block, headings as Markdown "#"
/// lines, or of an XML file, a line per element.
pub fn html_text(page: &str, xml: bool) -> String {
    let mut p = HtmlText {
        parts: Vec::new(),
        skip: 0,
        xml,
    };
    let mut rest = page;
    let mut text = String::new();
    let flush = |p: &mut HtmlText, text: &mut String| {
        if !text.is_empty() {
            p.data(&html_escape::decode_html_entities(text));
            text.clear();
        }
    };
    while let Some(lt) = rest.find('<') {
        text.push_str(&rest[..lt]);
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            flush(&mut p, &mut text);
            rest = body.find("-->").map(|i| &body[i + 3..]).unwrap_or("");
        } else if let Some(body) = rest.strip_prefix("<![") {
            flush(&mut p, &mut text);
            let (inner, after) = body.split_once("]]>").unwrap_or((body, ""));
            if let Some(cdata) = inner.strip_prefix("CDATA[") {
                p.data(cdata);
            }
            rest = after;
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            flush(&mut p, &mut text);
            rest = rest.find('>').map(|i| &rest[i + 1..]).unwrap_or("");
        } else if let Some((name, _)) = rest[1..].strip_prefix('/').and_then(tag_name) {
            flush(&mut p, &mut text);
            p.end(&name);
            rest = rest.find('>').map(|i| &rest[i + 1..]).unwrap_or("");
        } else if let Some((name, _)) = tag_name(&rest[1..]) {
            flush(&mut p, &mut text);
            let Some(end) = tag_end(rest) else {
                rest = "";
                break;
            };
            let self_closing = rest[..end].trim_end_matches('>').ends_with('/');
            rest = &rest[end..];
            p.start(&name);
            if self_closing {
                p.end(&name);
            } else if name == "script" || name == "style" {
                // Raw text up to the closing tag, as HTMLParser reads it.
                let close = format!("</{name}");
                // On the bytes: lowering case changes some characters' length.
                let at = rest
                    .as_bytes()
                    .windows(close.len())
                    .position(|w| w.eq_ignore_ascii_case(close.as_bytes()))
                    .unwrap_or(rest.len());
                p.data(&rest[..at]);
                rest = &rest[at..];
            }
        } else {
            text.push('<');
            rest = &rest[1..];
        }
    }
    text.push_str(rest);
    flush(&mut p, &mut text);
    let joined = p.parts.concat();
    splitlines(&joined)
        .into_iter()
        .map(fold_spaces)
        .filter(|line| line.chars().any(|c| c != '#' && c != ' '))
        .collect::<Vec<_>>()
        .join("\n")
}

/// An epub's chapters in reading order.
fn epub_text<R: Read + Seek>(pkg: &mut Package<R>) -> Result<String, String> {
    let container = pkg.xml("META-INF/container.xml")?;
    let opf = container
        .iter_tag(&ns(EPUB_CONTAINER, "rootfile"))
        .first()
        .map(|r| r.get("full-path").unwrap_or("").to_string())
        .unwrap_or_default();
    let book = pkg.xml(&opf)?;
    let hrefs: HashMap<String, String> = book
        .iter_tag(&ns(OPF, "item"))
        .into_iter()
        .map(|i| {
            (
                i.get("id").unwrap_or("").to_string(),
                i.get("href").unwrap_or("").to_string(),
            )
        })
        .collect();
    let folder = opf.rsplit_once('/').map(|(f, _)| f).unwrap_or("");
    let mut chapters = Vec::new();
    for r in book.iter_tag(&ns(OPF, "itemref")) {
        let href = hrefs
            .get(r.get("idref").unwrap_or(""))
            .map(String::as_str)
            .unwrap_or("");
        let page = pkg.read(&member(folder, href))?;
        let text = html_text(&String::from_utf8_lossy(&page), false);
        if !text.is_empty() {
            chapters.push(text);
        }
    }
    Ok(chapters.join("\n"))
}

/// An address list as an email header shows it: "Name <address>, address",
/// a group as "Name: address, address;".
fn addresses(list: Option<mail_parser::AddressList>) -> String {
    let mailbox = |m: mail_parser::Mailbox| match (m.name(), m.address()) {
        (Some(name), Some(address)) => format!("{name} <{address}>"),
        (None, Some(address)) => address.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => String::new(),
    };
    let Some(list) = list else {
        return String::new();
    };
    list.iter()
        .map(|a| match a {
            mail_parser::Address::Mailbox(m) => mailbox(m),
            mail_parser::Address::Group(g) => {
                let members = g.mailboxes().map(mailbox).collect::<Vec<_>>().join(", ");
                format!("{}: {members};", g.name().unwrap_or(""))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// An email: "# <subject>", its senders, recipients, date and attachments'
/// names, then its text, or its HTML's when it has no plain text.
fn eml_text(source: &mut dyn Read) -> Result<String, String> {
    let mut data = Vec::new();
    source
        .take(MAX_XML_BYTES)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    let msg = mail_parser::MessageParser::default()
        .parse(&data)
        .ok_or("the email could not be read.")?;
    let subject = msg
        .subject()
        .filter(|s| !s.is_empty())
        .unwrap_or("(no subject)");
    let mut lines = vec![format!("# {subject}")];
    let date = msg
        .headers()
        .get("Date")
        .map(|h| fold_spaces(&String::from_utf8_lossy(h.raw_value())))
        .unwrap_or_default();
    for (name, value) in [
        ("From", addresses(msg.from())),
        ("To", addresses(msg.to())),
        ("Cc", addresses(msg.cc())),
        ("Date", date),
    ] {
        if !value.is_empty() {
            lines.push(format!("{name}: {value}"));
        }
    }
    let attached: Vec<&str> = msg
        .attachments()
        .filter_map(|a| a.attachment_name())
        .collect();
    if !attached.is_empty() {
        lines.push(format!("Attachments: {}", attached.join(", ")));
    }
    if let Some(part) = msg.text_body().next().or_else(|| msg.html_body().next())
        && let Some(text) = part.text()
    {
        match part.kind() {
            mail_parser::PartKind::Html => lines.push(html_text(&text, false)),
            _ => lines.push(text.py_trim().to_string()),
        }
    }
    Ok(lines.join("\n"))
}

/// The text of a document in DOCUMENT_SUFFIXES.
pub fn document_text<R: Read + Seek>(source: R, suffix: &str) -> Result<String, String> {
    let mut source = source;
    if suffix == ".eml" {
        return eml_text(&mut source);
    }
    let zip = zip::ZipArchive::new(source).map_err(|e| format!("the document is damaged: {e}"))?;
    let mut pkg = Package {
        zip,
        left: MAX_XML_BYTES,
    };
    match suffix {
        ".docx" => Ok(paragraphs(&pkg.xml("word/document.xml")?)),
        ".odt" => Ok(paragraphs(&pkg.xml("content.xml")?)),
        ".pptx" => pptx_text(&mut pkg),
        ".odp" => odp_text(&mut pkg),
        ".xlsx" => xlsx_text(&mut pkg),
        ".ods" => ods_text(&mut pkg),
        ".epub" => epub_text(&mut pkg),
        _ => Err(format!("{suffix} is not a document.")),
    }
}

/// PDFium, loaded once from DOCS_PDFIUM or pdfium/lib/ beside bin/, as the
/// package unpacks it. It may not be called from two threads at once, even
/// on two documents.
fn pdfium() -> Result<&'static Mutex<Pdfium>, String> {
    static PDFIUM: OnceLock<Result<Mutex<Pdfium>, String>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let path = match std::env::var_os("DOCS_PDFIUM") {
                Some(p) => PathBuf::from(p),
                None => std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .parent()
                    .and_then(|bin| bin.parent())
                    .map(|dir| dir.join("pdfium/lib/libpdfium.so"))
                    .ok_or("no folder above the binary")?,
            };
            let bindings = Pdfium::bind_to_library(&path)
                .map_err(|e| format!("PDFium at {}: {e}", path.display()))?;
            Ok(Mutex::new(Pdfium::new(bindings)))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// (the PDF's page count, the text of count pages from first, or of all). A
/// word cut at a line end is joined, its hyphen kept before a capital.
pub fn pdf_pages<R: Read + Seek + Send + 'static>(
    source: R,
    first: usize,
    count: Option<usize>,
) -> Result<(usize, Vec<String>), String> {
    let lock = pdfium()?;
    let pdfium = lock.lock().unwrap_or_else(|e| e.into_inner());
    let doc = pdfium
        .load_pdf_from_reader(source, None)
        .map_err(|e| match e {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                "the PDF is password-protected.".to_string()
            }
            _ => "the PDF is damaged.".to_string(),
        })?;
    let pages = doc.pages();
    let total = pages.len() as usize;
    let end = count.map_or(total, |c| total.min(first.saturating_sub(1) + c));
    let mut texts = Vec::new();
    for n in first.saturating_sub(1)..end {
        let page = pages
            .get(n as i32)
            .map_err(|_| "the PDF is damaged.".to_string())?;
        let bounds = page
            .boundaries()
            .bounding()
            .map(|b| b.bounds)
            .unwrap_or_else(|_| page.page_size());
        let text = page
            .text()
            .map(|t| t.inside_rect(bounds))
            .unwrap_or_default();
        texts.push(join_cuts(&text.replace("\r\n", "\n")));
    }
    Ok((total, texts))
}

/// PDFium joins a word cut at a line end and marks the cut with \x02.
fn join_cuts(text: &str) -> String {
    static PDF_CUT: OnceLock<Regex> = OnceLock::new();
    let re = PDF_CUT.get_or_init(|| Regex::new(r"(?s)\x02(?:\r?\n)?(.?)").unwrap());
    re.replace_all(text, |c: &regex::Captures| {
        let next = &c[1];
        if next.chars().any(char::is_lowercase) && !next.chars().any(char::is_uppercase) {
            next.to_string()
        } else {
            format!("-{next}")
        }
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_as_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd\n"), vec!["a", "b", "c", "", "d"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
    }

    #[test]
    fn html_by_its_text() {
        let page = "<html><head><title>titre</title><style>.x { color: red }</style></head><body><h1>Tarte</h1>\
                    <p>P&acirc;te <b>bris\u{e9}e</b> maison</p><script>var cache = 1;</script></body></html>";
        assert_eq!(html_text(page, false), "# Tarte\nPâte brisée maison");
        let xml = r#"<carnet><plante nom="x"><![CDATA[Semis de capucines]]></plante><plante>Bouture de figuier</plante></carnet>"#;
        assert_eq!(
            html_text(xml, true),
            "Semis de capucines\nBouture de figuier"
        );
        assert_eq!(html_text("a < b <br/>c", false), "a < b\nc");
        // Characters whose lower case is longer or shorter.
        assert_eq!(
            html_text("<p>x</p><script>\u{e9}\u{212a}</SCRIPT>y", false),
            "x\ny"
        );
        let long = format!("<script>{}</script>z", "\u{130}".repeat(20));
        assert_eq!(html_text(&long, false), "z");
    }

    #[test]
    fn cuts_joined() {
        assert_eq!(join_cuts("recal\x02\nculee"), "recalculee");
        assert_eq!(join_cuts("Jean\x02\nPierre"), "Jean-Pierre");
    }

    #[test]
    fn members_as_posixpath() {
        assert_eq!(member("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(
            member("ppt", "/ppt/slides/slide2.xml"),
            "ppt/slides/slide2.xml"
        );
        assert_eq!(
            member("OEBPS", "Texte/chap%201.xhtml"),
            "OEBPS/Texte/chap 1.xhtml"
        );
        assert_eq!(member("xl", "../x.xml"), "x.xml");
    }
}
