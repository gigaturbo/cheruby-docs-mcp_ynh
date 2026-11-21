//! The tools: list, find, read and search one user's folders, and write in
//! those marked writable. Shared by the MCP server and the shell commands.
//!
//! Paths in and out of the tools start with a folder's label, never with a
//! real path. A path is refused if it climbs out of its folder, follows a
//! link out of it, or names a hidden file (Syncthing's .stfolder,
//! .stversions); one the folder's filters leave out is not found. Only
//! write_file and create_folder write, and only in the folders the folders
//! file marks writable: they create a folder, or create or replace a text
//! file, never delete anything, never through a link.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use chrono::{Local, TimeZone};
use rusqlite::{functions::FunctionFlags, types::Value as Sql};
use serde_json::Value;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use crate::{
    embed,
    folders::{
        Error, Item, TEXT_SUFFIXES, Tree, UserFolders, hidden, image_format, kept, py_repr, suffix,
    },
    index,
    nextcloud::Nextcloud,
    text::{DOCUMENT_SUFFIXES, PyTrim, document_text, pdf_pages, py_split},
};

const MAX_LIST: usize = 300;
const MAX_FOUND: usize = 50;
const MAX_CHARS: usize = 20_000;
const MAX_PDF_PAGES: usize = 20;
const MAX_IMAGE_BYTES: u64 = 4_000_000;
const MAX_FETCH_BYTES: u64 = 64_000_000;
const MAX_RESULTS: usize = 10;
const MAX_WRITE_BYTES: usize = 2_000_000;
const RRF_K: f64 = 60.0;
/// Measured on the Pi (F-M-12): the 30 best of the fusion, each by its start,
/// put the expected file in the top 5 for every test query in about 4 s. A
/// passage starts with its file name and headings (passages::passages).
const RERANK_CANDIDATES: usize = 30;
const RERANK_CHARS: usize = 360;
const MAX_RUN_FILES: usize = 50;
/// Requests waiting for the index unit; past this, run_index says so.
const MAX_REQUESTS: usize = 20;
/// How long run_index waits for a run of files to end.
const RUN_WAIT_S: u64 = 60;

/// What every tool reads: the folders, Nextcloud, the index and the servers.
pub struct Docs {
    pub users: BTreeMap<String, UserFolders>,
    pub nextcloud: Option<Nextcloud>,
    pub index_dir: PathBuf,
    pub embed_url: String,
    pub embed_model: String,
    pub query_prefix: &'static str,
    pub rerank_url: String,
    /// The index's quiet hours, such as 01-06, when run_index is refused.
    pub quiet: String,
    pub http: reqwest::Client,
}

/// What a token opens: one user, and all or some of their folders.
#[derive(Clone, Debug)]
pub struct Grant {
    pub user: String,
    pub folders: Vec<(String, String)>,
}

/// A tool call: what it reads, and who asks.
#[derive(Clone)]
pub struct Ctx {
    pub docs: Arc<Docs>,
    pub grant: Arc<Grant>,
}

/// What a tool returns: text, or a picture.
pub enum Reply {
    Text(String),
    Image { data: Vec<u8>, mime: String },
}

type ToolResult<T> = Result<T, String>;

impl Ctx {
    fn own(&self) -> &UserFolders {
        &self.docs.users[&self.grant.user]
    }

    fn labels(&self) -> Vec<&str> {
        self.grant.folders.iter().map(|(l, _)| l.as_str()).collect()
    }

    fn tree(&self) -> Tree<'_> {
        Tree::new(
            &self.grant.folders,
            &self.own().rules,
            self.docs.nextcloud.as_ref(),
        )
    }

    fn no_folder(&self, path: &str, first: &str) -> String {
        format!(
            "{path}: no folder named {}. Folders: {}.",
            py_repr(first),
            self.labels().join(", ")
        )
    }

    /// The Item behind a tool path, or an error. "" means the folder list, None.
    fn resolve(&self, path: &str) -> ToolResult<Option<Item>> {
        let parts = split(path);
        let Some(first) = parts.first() else {
            return Ok(None);
        };
        if parts
            .iter()
            .any(|p| p == ".." || hidden(p) || p.contains('\0'))
        {
            return Err(format!("{path}: hidden names and .. are not allowed."));
        }
        if !self.labels().contains(&first.as_str()) {
            return Err(self.no_folder(path, first));
        }
        // A filter also hides that a file exists: left out reads as missing.
        match self.tree().item(first, &parts[1..]) {
            Ok(Some(item)) => Ok(Some(item)),
            Ok(None) | Err(Error::NotFound) => Err(format!("{path}: not found.")),
            Err(e) => Err(format!("{path}: {e}")),
        }
    }
}

/// Runs blocking work, the folders or Nextcloud, off the event loop: a
/// Nextcloud that hangs must not stall the other requests.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ToolResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("internal error: {e}"))
}

pub fn split(path: &str) -> Vec<String> {
    path.py_trim()
        .trim_matches('/')
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(String::from)
        .collect()
}

fn date(ns: i64) -> String {
    Local.timestamp_nanos(ns).format("%Y-%m-%d").to_string()
}

/// One listing line: kind, size, date, path.
fn describe(item: &Item) -> String {
    if item.is_dir {
        return format!("dir             {}  {}/", date(item.mtime_ns), item.shown);
    }
    format!(
        "file {:>9}  {}  {}",
        human(item.size as f64),
        date(item.mtime_ns),
        item.shown
    )
}

pub fn human(mut size: f64) -> String {
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 || unit == "GB" {
            return if unit == "B" {
                format!("{size:.0} {unit}")
            } else {
                format!("{size:.1} {unit}")
            };
        }
        size /= 1024.0;
    }
    unreachable!()
}

/// Lower case without accents, so "factures energie" finds "Factures_Énergie".
fn fold(text: &str) -> String {
    text.to_lowercase()
        .nfkd()
        .filter(|c| canonical_combining_class(*c) == 0)
        .collect()
}

fn words_of(text: &str) -> String {
    fold(text).replace(['_', '-'], " ")
}

/// List one folder: its subfolders, then its files with size and date.
pub async fn list_folder(ctx: &Ctx, path: String) -> ToolResult<String> {
    let c = ctx.clone();
    blocking(move || {
        let Some(folder) = c.resolve(&path)? else {
            let own = c.own();
            return Ok(c
                .grant
                .folders
                .iter()
                .map(|(l, _)| {
                    format!(
                        "dir  {l}/{}",
                        if own.writable.contains(l) {
                            " (writable)"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"));
        };
        if !folder.is_dir {
            return Err(format!("{path}: is a file; read it with read_file."));
        }
        let mut items = c
            .tree()
            .children(&folder)
            .map_err(|e| format!("{path}: {e}"))?;
        items.sort_by_key(|i| (!i.is_dir, i.shown.to_lowercase()));
        let mut lines: Vec<String> = items.iter().take(MAX_LIST).map(describe).collect();
        if items.len() > MAX_LIST {
            lines.push(format!(
                "[{} more not shown; use find_files]",
                items.len() - MAX_LIST
            ));
        }
        if lines.is_empty() {
            return Ok(format!("{}/ is empty.", folder.shown));
        }
        Ok(lines.join("\n"))
    })
    .await?
}

/// Find files and folders whose path contains every word of the query.
pub async fn find_files(ctx: &Ctx, query: String, path: String) -> ToolResult<String> {
    let words: Vec<String> = py_split(&words_of(&query)).map(String::from).collect();
    if words.is_empty() {
        return Err("query is empty.".into());
    }
    let c = ctx.clone();
    blocking(move || {
        let start = c.resolve(&path)?;
        let mut tree = c.tree();
        let mut found = Vec::new();
        let mut notes = Vec::new();
        let mut starts = Vec::new();
        match start {
            Some(item) => starts.push(Some(item)),
            None => {
                for (label, _) in c.grant.folders.iter() {
                    match tree.item(label, &[]) {
                        Ok(top) => starts.push(top),
                        Err(e) => notes.push(format!("[{label}: {e}]")),
                    }
                }
            }
        }
        for top in starts.into_iter().flatten() {
            let items = match tree.walk(&top) {
                Ok(items) => items,
                Err(e) => {
                    notes.push(format!("[{}: {e}]", top.shown));
                    continue;
                }
            };
            for item in items {
                let name = words_of(&item.shown);
                if words.iter().all(|w| name.contains(w.as_str())) {
                    found.push(describe(&item));
                    if found.len() > MAX_FOUND {
                        return Ok(
                            found[..MAX_FOUND].join("\n") + "\n[more matches; narrow the query]"
                        );
                    }
                }
            }
        }
        notes.extend(
            tree.unlisted
                .iter()
                .map(|s| format!("[{s}: Nextcloud did not list it]")),
        );
        found.extend(notes);
        if found.is_empty() {
            return Ok(format!("Nothing matches {}.", py_repr(&query)));
        }
        Ok(found.join("\n"))
    })
    .await?
}

/// A slice of text with a note saying how to read on.
fn cut(text: &str, offset: usize, max_chars: usize) -> String {
    let total = text.chars().count();
    let mut part: String = text.chars().skip(offset).take(max_chars).collect();
    if offset + max_chars < total {
        let shown = part.chars().count();
        part += &format!(
            "\n\n[characters {offset}-{} of {total}; continue with offset={}]",
            offset + shown,
            offset + max_chars
        );
    }
    part
}

fn pdf_text(
    source: crate::folders::Source,
    page: i64,
    pages: usize,
) -> Result<Result<String, String>, String> {
    let (total, texts) = pdf_pages(
        source,
        page.max(1) as usize,
        Some(if page < 1 { 0 } else { pages }),
    )?;
    if page < 1 || page as usize > total {
        return Ok(Err(format!(
            "page {page} is out of range; the PDF has {total} pages."
        )));
    }
    let page = page as usize;
    let last = page + texts.len() - 1;
    let chunks: Vec<String> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let t = t.py_trim();
            let t = if t.is_empty() {
                "[no text on this page: a scan or an image]"
            } else {
                t
            };
            format!("--- page {} ---\n{t}", page + i)
        })
        .collect();
    let mut head = format!("{total} pages; pages {page}-{last} below.");
    if last < total {
        head += &format!(" Continue with page={}.", last + 1);
    }
    Ok(Ok(format!("{head}\n\n{}", chunks.join("\n\n"))))
}

/// Read one file: text, Markdown, PDF, Word, Excel, PowerPoint, OpenDocument,
/// EPUB, email (.eml) or an image.
pub async fn read_file(
    ctx: &Ctx,
    path: String,
    page: i64,
    pages: i64,
    offset: i64,
    max_chars: i64,
) -> ToolResult<Reply> {
    let c = ctx.clone();
    blocking(move || {
        let item = match c.resolve(&path)? {
            Some(item) if !item.is_dir => item,
            _ => {
                let shown = if path.is_empty() { "(empty)" } else { &path };
                return Err(format!("{shown}: is a folder; list it with list_folder."));
            }
        };
        let ext = suffix(&item.shown);
        let image = image_format(&ext);
        let readable = image.is_some()
            || DOCUMENT_SUFFIXES.contains(&ext.as_str())
            || TEXT_SUFFIXES.contains(&ext.as_str())
            || ext == ".pdf";
        if !readable {
            let what = if ext.is_empty() {
                "this file type"
            } else {
                &ext
            };
            return Err(format!("{path}: {what} cannot be read as text."));
        }
        if image.is_some() && item.size as u64 > MAX_IMAGE_BYTES {
            return Err(format!(
                "{path}: the image is larger than {}.",
                human(MAX_IMAGE_BYTES as f64)
            ));
        }
        let pages = pages.clamp(1, MAX_PDF_PAGES as i64) as usize;
        let offset = offset.max(0) as usize;
        let max_chars = max_chars.clamp(1, MAX_CHARS as i64) as usize;
        let limit = if image.is_some() {
            MAX_IMAGE_BYTES
        } else {
            MAX_FETCH_BYTES
        };
        let failed = |e: String| format!("{path}: {e}");
        let mut f = c.tree().open(&item, limit).map_err(|e| match e {
            Error::NotFound => format!("{path}: not found."),
            e => failed(e.to_string()),
        })?;
        use std::io::Read;
        if let Some(format) = image {
            let mut data = Vec::new();
            f.read_to_end(&mut data)
                .map_err(|e| failed(crate::folders::io_message(&e)))?;
            return Ok(Reply::Image {
                data,
                mime: format!("image/{format}"),
            });
        }
        if ext == ".pdf" {
            return pdf_text(f, page, pages).map_err(failed)?.map(Reply::Text);
        }
        if DOCUMENT_SUFFIXES.contains(&ext.as_str()) {
            return Ok(Reply::Text(cut(
                &document_text(f, &ext).map_err(failed)?,
                offset,
                max_chars,
            )));
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| failed(crate::folders::io_message(&e)))?;
        Ok(Reply::Text(cut(
            &String::from_utf8_lossy(&data),
            offset,
            max_chars,
        )))
    })
    .await?
}

/// Local midnight of a day such as 2025-03-31, in nanoseconds.
fn day(text: &str, name: &str) -> ToolResult<i64> {
    let bad = || format!("{name}: {} is not a date like 2025-03-31.", py_repr(text));
    let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| bad())?;
    let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
    let local = Local
        .from_local_datetime(&midnight)
        .earliest()
        .ok_or_else(bad)?;
    Ok(local.timestamp() * 1_000_000_000)
}

/// What the index holds for a query, for search to order and show.
struct Found {
    meta: BTreeMap<String, String>,
    rebuilding: bool,
    ids: Vec<i64>,
    /// id: (path, page, mtime_ns, text)
    rows: BTreeMap<i64, (String, i64, i64, String)>,
}

/// Passage ids, best first, by meaning and by keywords, merged by reciprocal rank.
fn ranked(
    db: &rusqlite::Connection,
    query: &str,
    vector: Option<&[f32]>,
    clause: &str,
    args: &[Sql],
) -> rusqlite::Result<Vec<i64>> {
    let mut scores: Vec<(i64, f64)> = Vec::new();
    if let Some(vector) = vector {
        let sql =
            format!("SELECT p.id, p.vector FROM passages p JOIN files f ON f.id = p.file {clause}");
        let mut stmt = db.prepare(&sql)?;
        let rows: Vec<(i64, f32)> = stmt
            .query_map(rusqlite::params_from_iter(args), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    embed::dot(&embed::from_blob(&r.get::<_, Vec<u8>>(1)?), vector),
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            rows[b]
                .1
                .partial_cmp(&rows[a].1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (rank, i) in order.into_iter().take(50).enumerate() {
            scores.push((rows[i].0, 1.0 / (RRF_K + rank as f64)));
        }
    }
    let words: Vec<&str> = py_split(query)
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    if !words.is_empty() {
        let fts = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT p.id FROM passages_fts JOIN passages p ON p.id = passages_fts.rowid \
             JOIN files f ON f.id = p.file {clause} AND passages_fts MATCH ? ORDER BY bm25(passages_fts) LIMIT 50"
        );
        let mut all = args.to_vec();
        all.push(Sql::Text(fts));
        let mut stmt = db.prepare(&sql)?;
        let ids: Vec<i64> = stmt
            .query_map(rusqlite::params_from_iter(&all), |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for (rank, id) in ids.into_iter().enumerate() {
            let score = 1.0 / (RRF_K + rank as f64);
            match scores.iter_mut().find(|(i, _)| *i == id) {
                Some((_, s)) => *s += score,
                None => scores.push((id, score)),
            }
        }
    }
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scores.into_iter().map(|(id, _)| id).collect())
}

/// Find passages of documents by what they say, in the user's words.
pub async fn search_documents(
    ctx: &Ctx,
    query: String,
    path: String,
    after: String,
    before: String,
    limit: i64,
) -> ToolResult<String> {
    let query = query.py_trim().to_string();
    if query.is_empty() {
        return Err("query is empty.".into());
    }
    let db_path = ctx
        .docs
        .index_dir
        .join(format!("{}.sqlite", ctx.grant.user));
    if !db_path.exists() {
        return Err("the search index is not built yet; use find_files meanwhile.".into());
    }
    let parts = split(&path);
    if parts.iter().any(|p| p == ".." || hidden(p)) {
        return Err(format!("{path}: hidden names and .. are not allowed."));
    }
    if let Some(first) = parts.first()
        && !ctx.labels().contains(&first.as_str())
    {
        return Err(ctx.no_folder(&path, first));
    }
    // The index holds all the user's folders; a token may open only some of them.
    let labels = ctx.labels();
    let mut clauses = vec![
        "f.status = 'indexed'".to_string(),
        "kept(f.path)".to_string(),
        format!(
            "substr(f.path, 1, instr(f.path, '/') - 1) IN ({})",
            vec!["?"; labels.len()].join(",")
        ),
    ];
    let mut args: Vec<Sql> = labels.iter().map(|l| Sql::Text(l.to_string())).collect();
    if !parts.is_empty() {
        let prefix = parts.join("/");
        clauses.push("(f.path = ? OR substr(f.path, 1, ?) = ?)".into());
        args.push(Sql::Text(prefix.clone()));
        args.push(Sql::Integer(prefix.chars().count() as i64 + 1));
        args.push(Sql::Text(format!("{prefix}/")));
    }
    if !after.is_empty() {
        clauses.push("f.mtime_ns >= ?".into());
        args.push(Sql::Integer(day(&after, "after")?));
    }
    if !before.is_empty() {
        clauses.push("f.mtime_ns < ?".into());
        args.push(Sql::Integer(day(&before, "before")? + 86_400_000_000_000));
    }
    let clause = format!("WHERE {}", clauses.join(" AND "));

    let vector = embed::embed_query(
        &ctx.docs.http,
        &ctx.docs.embed_url,
        &format!("{}{query}", ctx.docs.query_prefix),
    )
    .await;
    let c = ctx.clone();
    let q = query.clone();
    let asked = vector.clone();
    let found = blocking(move || -> ToolResult<Found> {
        let failed = |e: rusqlite::Error| format!("the search index could not be read: {e}");
        let db = index::connect(&db_path, true).map_err(failed)?;
        // Passages of files a filter now leaves out, until the next index run drops them.
        let rules = c.own().rules.clone();
        db.create_scalar_function("kept", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, move |f| {
            Ok(kept(&rules, &f.get::<String>(0)?, false))
        })
        .map_err(failed)?;
        let meta: BTreeMap<String, String> = db
            .prepare("SELECT key, value FROM meta")
            .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .map_err(failed)?;
        // A label now on another path: its text in the index is not what read_file
        // reaches. A folder the index does not know yet just has no passages.
        let indexed: BTreeMap<String, Value> = meta.get("folders").and_then(|f| serde_json::from_str(f).ok()).unwrap_or_default();
        if c.grant.folders.iter().any(|(l, real)| indexed.get(l).is_some_and(|v| v.as_str() != Some(real))) {
            return Err("the search index is being rebuilt after a change of folders; use find_files meanwhile.".into());
        }
        // Vectors of another model: comparing them with this query means nothing,
        // and fails outright when their sizes differ.
        let rebuilding = !meta.get("model").is_some_and(|m| m.starts_with(&format!("{}/", c.docs.embed_model)));
        let vector = if rebuilding { None } else { asked };
        let mut ids = ranked(&db, &q, vector.as_deref(), &clause, &args).map_err(failed)?;
        ids.truncate(50);
        let sql = format!(
            "SELECT p.id, f.path, p.page, f.mtime_ns, p.text FROM passages p JOIN files f ON f.id = p.file WHERE p.id IN ({})",
            vec!["?"; ids.len()].join(",")
        );
        let rows = db
            .prepare(&sql)
            .and_then(|mut s| {
                s.query_map(rusqlite::params_from_iter(&ids), |r| {
                    Ok((r.get(0)?, (r.get(1)?, r.get::<_, Option<i64>>(2)?.unwrap_or(0), r.get(3)?, r.get(4)?)))
                })?
                .collect()
            })
            .map_err(failed)?;
        Ok(Found { meta, rebuilding, ids, rows })
    })
    .await??;
    let rebuilding = found.rebuilding;
    let meaning = !rebuilding && vector.is_some();
    let rows = &found.rows;
    // Overlapping passages of one page are one candidate, its best one.
    let mut seen = std::collections::HashSet::new();
    let mut pages: Vec<i64> = found
        .ids
        .iter()
        .copied()
        .filter(|id| seen.insert((rows[id].0.clone(), rows[id].1)))
        .collect();
    let mut reranked = None;
    if !ctx.docs.rerank_url.is_empty() && pages.len() > 1 {
        let docs: Vec<String> = pages
            .iter()
            .take(RERANK_CANDIDATES)
            .map(|id| rows[id].3.chars().take(RERANK_CHARS).collect())
            .collect();
        reranked = embed::rerank(&ctx.docs.http, &ctx.docs.rerank_url, &query, &docs).await;
        if let Some(order) = &reranked {
            let head: Vec<i64> = order.iter().map(|&i| pages[i]).collect();
            let tail = pages.split_off(docs.len());
            pages = head;
            pages.extend(tail);
        }
    }
    // The best passage stands for its file, so one long document cannot fill the list.
    let mut files = std::collections::HashSet::new();
    let best: Vec<i64> = pages
        .iter()
        .copied()
        .filter(|id| files.insert(rows[id].0.clone()))
        .collect();
    let limit = limit.clamp(1, MAX_RESULTS as i64) as usize;
    let mut head = format!(
        "[index updated {}]\n",
        found.meta.get("updated").map(String::as_str).unwrap_or("?")
    );
    if rebuilding {
        head += "[the index is being rebuilt for a new embedding model; keyword matches only]\n";
    } else if !meaning {
        head += "[search by meaning is unavailable; keyword matches only]\n";
    }
    if !ctx.docs.rerank_url.is_empty() && pages.len() > 1 && reranked.is_none() {
        head += "[ranking by relevance is unavailable; results in a rougher order]\n";
    }
    head += "\n";
    if best.is_empty() {
        return Ok(format!("{head}Nothing matches {}.", py_repr(&query)));
    }
    let blocks: Vec<String> = best
        .iter()
        .take(limit)
        .map(|id| {
            let (shown, page, mtime_ns, text) = &rows[id];
            let place = if *page != 0 {
                format!("{shown}, page {page}")
            } else {
                shown.clone()
            };
            format!("--- {place} ({}) ---\n{text}", date(*mtime_ns))
        })
        .collect();
    Ok(head + &blocks.join("\n\n"))
}

/// "indexed", or what else the published index says of a file.
fn indexed_status(db: &rusqlite::Connection, shown: &str) -> String {
    let row = db.query_row(
        "SELECT status, error FROM files WHERE path = ?",
        [shown],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    );
    match row {
        Ok((status, Some(error))) if status == "failed" => format!("failed: {error}"),
        Ok((status, _)) if status == "needs_ocr" => "waiting for OCR, not searchable yet".into(),
        Ok((status, _)) => status,
        Err(_) => "not in the index".into(),
    }
}

/// Ask the index unit for a run of these files, or of all the folders.
///
/// The server cannot write the index or start a unit: it leaves a request
/// in <index dir>/requests, the one place it may write there, and the
/// index's path unit starts a run. Refused in the quiet hours, and when
/// MAX_REQUESTS wait already. A run of files is waited for, up to RUN_WAIT_S.
pub async fn run_index(ctx: &Ctx, files: Vec<String>) -> ToolResult<String> {
    if files.len() > MAX_RUN_FILES {
        return Err(format!(
            "at most {MAX_RUN_FILES} files at a time; an empty list asks for all the folders."
        ));
    }
    let c = ctx.clone();
    let shown = blocking(move || -> ToolResult<Vec<String>> {
        let mut shown = Vec::new();
        for path in files {
            match c.resolve(&path)? {
                Some(item) if !item.is_dir => shown.push(item.shown),
                _ => {
                    return Err(format!(
                        "{path}: is a folder; name its files, or ask for all the folders with an empty list."
                    ));
                }
            }
        }
        Ok(shown)
    })
    .await??;
    let quiet = &ctx.docs.quiet;
    if index::quiet_now(quiet) {
        let end = quiet.split('-').nth(1).unwrap_or("").trim();
        return Err(format!(
            "the index does not run in its quiet hours, {quiet}; the next run after {end}:00 picks the changes up."
        ));
    }
    let spool = ctx.docs.index_dir.join("requests");
    let waiting = std::fs::read_dir(&spool)
        .map_err(|e| {
            format!(
                "the index takes no request: {}",
                crate::folders::io_message(&e)
            )
        })?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count();
    if waiting >= MAX_REQUESTS {
        return Err(format!(
            "{waiting} index requests are waiting already; the next run takes them all."
        ));
    }
    let name = format!(
        "{}-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id()
    );
    let request = spool.join(format!("{name}.json"));
    // Named .json only once whole: the path unit and the indexer look at .json files.
    let tmp = spool.join(format!(".{name}.tmp"));
    let body = serde_json::json!({"user": ctx.grant.user, "files": shown});
    std::fs::write(&tmp, body.to_string())
        .and_then(|()| std::fs::rename(&tmp, &request))
        .map_err(|e| {
            format!(
                "the request could not be left: {}",
                crate::folders::io_message(&e)
            )
        })?;
    let base = ctx.docs.index_dir.join(&ctx.grant.user);
    if shown.is_empty() {
        let status = index::read_status(&base);
        let state = if status.get("result").and_then(Value::as_str) == Some("running") {
            "activating"
        } else {
            "inactive"
        };
        let unit = "cheruby-docs-mcp-index";
        return Ok(format!(
            "An index run of all the folders is asked for; it redoes only what changed.\n{}",
            crate::status::render(&status, state, None, unit)
        ));
    }
    // Taken by the indexer, then its run ended: the published index says how each went.
    for _ in 0..RUN_WAIT_S {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let running = index::read_status(&base)
            .get("result")
            .and_then(Value::as_str)
            == Some("running");
        if request.exists() || running {
            continue;
        }
        let db_path = base.with_extension("sqlite");
        let lines = blocking(move || -> ToolResult<Vec<String>> {
            let db = index::connect(&db_path, true)
                .map_err(|e| format!("the search index could not be read: {e}"))?;
            Ok(shown
                .iter()
                .map(|s| format!("- {s}: {}", indexed_status(&db, s)))
                .collect())
        })
        .await??;
        return Ok(format!("The index run is done.\n{}", lines.join("\n")));
    }
    Ok(format!(
        "The run is asked for, and has not ended within {RUN_WAIT_S} s: another run may be going. \
         Search finds the files once it ends."
    ))
}

/// The parts of a path in a folder this token may write in, or an error.
fn writable_parts(ctx: &Ctx, path: &str) -> ToolResult<Vec<String>> {
    let parts = split(path);
    if parts
        .iter()
        .any(|p| p == ".." || hidden(p) || p.contains('\0'))
    {
        return Err(format!("{path}: hidden names and .. are not allowed."));
    }
    let own = ctx.own();
    let writable: Vec<&str> = ctx
        .labels()
        .into_iter()
        .filter(|l| own.writable.contains(*l))
        .collect();
    let listed = if writable.is_empty() {
        "none".to_string()
    } else {
        writable.join(", ")
    };
    if parts.len() < 2 {
        let shown = if path.is_empty() { "(empty)" } else { path };
        return Err(format!(
            "{shown}: name something inside a writable folder: {listed}."
        ));
    }
    if !ctx.labels().contains(&parts[0].as_str()) {
        return Err(format!(
            "{path}: no folder named {}. Writable folders: {listed}.",
            py_repr(&parts[0])
        ));
    }
    if !writable.contains(&parts[0].as_str()) {
        return Err(format!(
            "{} is read-only. Writable folders: {listed}.",
            parts[0]
        ));
    }
    Ok(parts)
}

/// What write_file returns, or, with content None, create_folder.
pub async fn write(ctx: &Ctx, path: String, content: Option<String>) -> ToolResult<String> {
    let parts = writable_parts(ctx, &path)?;
    let data = match content {
        None => None,
        Some(content) => {
            let ext = suffix(parts.last().map(String::as_str).unwrap_or(""));
            if !TEXT_SUFFIXES.contains(&ext.as_str()) {
                let mut known: Vec<&str> = TEXT_SUFFIXES.to_vec();
                known.sort_unstable();
                return Err(format!(
                    "{path}: only text files can be written: {}.",
                    known.join(", ")
                ));
            }
            if content.len() > MAX_WRITE_BYTES {
                return Err(format!(
                    "{path}: the content is larger than {}.",
                    human(MAX_WRITE_BYTES as f64)
                ));
            }
            Some(content.into_bytes())
        }
    };
    let c = ctx.clone();
    let p = parts.clone();
    let size = data.as_ref().map(Vec::len);
    let outcome = blocking(move || c.tree().write(&p[0], &p[1..], data.as_deref()))
        .await?
        .map_err(|e| format!("{path}: {e}"))?;
    let shown = parts.join("/");
    Ok(match (outcome, size) {
        ("exists", _) => format!("{shown}/ already exists."),
        (_, None) => format!("Created {shown}/."),
        (outcome, Some(size)) => {
            let mut word = outcome.to_string();
            word[..1].make_ascii_uppercase();
            format!("{word} {shown}, {}.", human(size as f64))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_as_python() {
        assert_eq!(human(10.0), "10 B");
        assert_eq!(human(2_000_000.0), "1.9 MB");
        assert_eq!(human(4_000_000.0), "3.8 MB");
    }

    #[test]
    fn folded_without_accents() {
        assert_eq!(words_of("Factures_Énergie-2024"), "factures energie 2024");
    }

    #[test]
    fn cut_says_how_to_read_on() {
        assert_eq!(
            cut("abcdef", 0, 4),
            "abcd\n\n[characters 0-4 of 6; continue with offset=4]"
        );
        assert_eq!(cut("abcdef", 4, 4), "ef");
    }
}
