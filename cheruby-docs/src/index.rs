//! Search index: each user's documents cut into passages, with their vectors.
//!
//! Run by cheruby-docs-mcp-index.service, on its timer. Settings come from
//! its root-600 env file, which holds no MCP token:
//!
//!     DOCS_FOLDERS_FILE   as for the tool server, and DOCS_NEXTCLOUD_* when a folder is in Nextcloud
//!     DOCS_INDEX_DIR      where each user's index is written
//!     DOCS_EMBED_URL      the embedding server, e.g. http://127.0.0.1:8771
//!     DOCS_EMBED_MODEL    the model's identity; when it changes, everything is redone
//!     DOCS_EMBED_NAME     the model's family (embed::passage)
//!     DOCS_QUIET_HOURS    optional, such as 01-06: no run in these hours, and one
//!                         still going stops when they begin. A file run-now in
//!                         the index folder, left by the panel's Run now, lifts
//!                         this for one run.
//!     DOCS_REMOTE_URL     optional: Whoopy on loopback, e.g. http://127.0.0.1:4000
//!     DOCS_REMOTE_MODEL   the model there, as Whoopy names it
//!     DOCS_REMOTE_KEY     a Whoopy key allowed that model only
//!
//! A run-now file holding "remote", left by the panel's "Run now on the
//! desktop", sends the passages to Whoopy instead of the embedding server on
//! the Pi. The vectors are stored under the Pi's model id, since search
//! embeds queries on the Pi: so the run first embeds CHECK_PASSAGES on both
//! and keeps to the Pi unless every pair agrees to MIN_COSINE. When Whoopy
//! stops answering, the run goes on on the Pi. The timer never leaves the
//! file, so it never uses Whoopy.
//!
//! Each run walks the folders and redoes only the files whose size or dates
//! changed; a moved file keeps its passages, a deleted one loses them, and so
//! do the files of a folder taken away or pointed elsewhere, and those its
//! filters now leave out. A file that cannot be read is marked failed and
//! retried once it changes; the run goes on. A Nextcloud folder is listed
//! only where its ETag changed since the last run; when Nextcloud does not
//! answer, what the index holds of it stays, and the run ends as failed.
//! Text comes from the files' own text: scans and images are marked as
//! waiting for OCR and hold no passage. Paths are stored as the tools show
//! them (Documents/...), never as real paths.
//!
//! The indexer works on <user>.work.sqlite and publishes a finished copy as
//! <user>.sqlite every ten minutes and at the end, so the tool server, which
//! only reads, never sees a write half done. Deleting the working copy makes
//! the next run start from zero; search goes on with the published one until
//! its first publish.
//!
//! <user>.status.json, for the config panel: when the last run started and
//! ended, how it ended, what it did, and the index's files by status.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    embed::{self, EmbedError, Embedder},
    folders::{self, Error, Item, Source, TEXT_SUFFIXES, Tree, UserFolders, image_format, suffix},
    nextcloud::{Entry, Nextcloud, hex},
    passages::{PASSAGES_VERSION, passages, without_running_lines},
    text::{DOCUMENT_SUFFIXES, PyTrim, document_text, html_text, pdf_pages},
};

const MAX_FILE_CHARS: usize = 300_000;
const MAX_FETCH_BYTES: u64 = 64_000_000;
pub const MIN_COSINE: f32 = 0.995;
/// The server or the gateway refuses, is down, or holds requests back: not the
/// file's fault, so the run stops instead of marking every file failed.
const SERVER_REFUSED: &[u16] = &[401, 403, 429, 502, 503, 504];
const PUBLISH_EVERY_S: u64 = 600;

/// Embedded on the Pi and on the desktop before a remote run. Generic text in
/// both languages and the shapes the folders hold: prose, a list, a table,
/// code, a heading line, a long paragraph.
const CHECK_PASSAGES: &[&str] = &[
    "Le conseil municipal a voté le budget de l'année prochaine après deux heures de débat.",
    "The meeting was moved to Thursday because half the team was travelling.",
    "Recette de la pâte brisée : 250 g de farine, 125 g de beurre, une pincée de sel, 5 cl d'eau.",
    "Invoice 2041: due within 30 days. Late payments incur a fee of 1.5% per month.",
    "notes.md > Courses\n- lait\n- pain\n- tomates\n- café",
    "def mean(values):\n    return sum(values) / len(values) if values else 0.0",
    "Le contrat de location prend fin le 30 juin ; le préavis est de trois mois pour le locataire.",
    "Photosynthesis converts light energy into chemical energy stored in glucose.",
    "| Mois | Électricité | Gaz |\n| janvier | 82 € | 64 € |\n| février | 77 € | 59 € |",
    "Rendez-vous chez le dentiste mardi à 14 h 30, apporter la carte vitale.",
    "The warranty covers manufacturing defects for two years from the date of purchase.",
    "rapport.pdf > Introduction > Contexte",
    "Après la tempête, les arbres tombés ont bloqué la route départementale pendant trois jours, \
     et les habitants du hameau ont dû passer par le chemin forestier pour rejoindre le village. \
     La mairie a fait venir une entreprise d'élagage, puis les services de l'État ont rouvert la voie.",
    "Abstract: we study how retrieval quality changes when passages are cut at headings rather than \
     at a fixed length, on three collections of notes, reports and scientific articles.",
    "Mot de passe Wi-Fi changé, penser à mettre à jour l'imprimante et la télévision.",
    "SELECT name, count(*) FROM orders GROUP BY name ORDER BY 2 DESC;",
    "Chapter 3. The lighthouse keeper had not seen a ship in eleven days.",
    "Attestation d'assurance habitation, valable du 1er janvier au 31 décembre.",
    "To reset the router, hold the button for ten seconds until the light blinks orange.",
    "Liste des pièces : vis M4 × 20, rondelles, écrous, clé Allen de 3 mm.",
];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, size INTEGER, mtime_ns INTEGER,
    ctime_ns INTEGER, sha256 TEXT, status TEXT, empty_pages INTEGER DEFAULT 0, error TEXT);
CREATE TABLE IF NOT EXISTS passages (
    id INTEGER PRIMARY KEY, file INTEGER NOT NULL, page INTEGER, text TEXT, vector BLOB);
CREATE INDEX IF NOT EXISTS passages_file ON passages (file);
CREATE TABLE IF NOT EXISTS nextcloud_folders (path TEXT PRIMARY KEY, listing TEXT);
CREATE VIRTUAL TABLE IF NOT EXISTS passages_fts USING fts5(
    text, content='passages', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
CREATE TRIGGER IF NOT EXISTS passages_ai AFTER INSERT ON passages BEGIN
    INSERT INTO passages_fts (rowid, text) VALUES (new.id, new.text); END;
CREATE TRIGGER IF NOT EXISTS passages_ad AFTER DELETE ON passages BEGIN
    INSERT INTO passages_fts (passages_fts, rowid, text) VALUES ('delete', old.id, old.text); END;
";

/// The index at path. Read-only opens never create or change it.
pub fn connect(path: &Path, read_only: bool) -> rusqlite::Result<Connection> {
    let db = if read_only {
        Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?
    } else {
        Connection::open(path)?
    };
    db.busy_timeout(std::time::Duration::from_secs(10))?;
    if !read_only {
        db.execute_batch(SCHEMA)?;
    }
    Ok(db)
}

/// <base>.status.json, or an empty object when missing or unreadable.
pub fn read_status(base: &Path) -> Map<String, Value> {
    std::fs::read_to_string(status_path(base))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn status_path(base: &Path) -> std::path::PathBuf {
    base.with_extension("status.json")
}

/// Merge fields into <base>.status.json in one step.
fn write_status(base: &Path, fields: Value) {
    let mut status = read_status(base);
    if let Value::Object(fields) = fields {
        status.extend(fields);
    }
    let path = status_path(base);
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::write(&tmp, Value::Object(status).to_string())
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        eprintln!("{}: {e}", path.display());
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// Whether the hour is within quiet hours "<from>-<to>", to excluded.
pub fn quiet_now(quiet: &str) -> bool {
    use chrono::Timelike;
    quiet_at(quiet, chrono::Local::now().hour())
}

pub fn quiet_at(quiet: &str, hour: u32) -> bool {
    let Some((a, b)) = quiet.split_once('-') else {
        return false;
    };
    let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) else {
        return false;
    };
    let (a, b) = (a % 24, b % 24);
    if a < b {
        a <= hour && hour < b
    } else {
        hour >= a || hour < b
    }
}

/// Copy the working index to <base>.sqlite in one step, stamped with the
/// time, and its files by status to the status file.
fn publish(db: &Connection, base: &Path) -> rusqlite::Result<()> {
    db.execute("INSERT OR REPLACE INTO meta VALUES ('updated', ?)", [now()])?;
    let tmp = base.with_extension("sqlite.tmp");
    db.backup(
        rusqlite::MAIN_DB,
        &tmp,
        None::<fn(rusqlite::backup::Progress)>,
    )?;
    std::fs::rename(&tmp, base.with_extension("sqlite"))
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let mut files = Map::new();
    let mut stmt = db.prepare("SELECT status, count(*) FROM files GROUP BY status")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (status, count) = row?;
        files.insert(status, json!(count));
    }
    write_status(base, json!({"files": files}));
    Ok(())
}

fn drop_file(db: &Connection, id: i64) -> rusqlite::Result<()> {
    db.execute("DELETE FROM passages WHERE file = ?", [id])?;
    db.execute("DELETE FROM files WHERE id = ?", [id])?;
    Ok(())
}

/// The hash of a file, which is left at its start.
fn sha256(f: &mut Source) -> std::io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    f.seek(SeekFrom::Start(0))?;
    Ok(hex(&h.finalize()))
}

fn take_chars(text: &str, n: usize) -> String {
    match text.char_indices().nth(n) {
        Some((i, _)) => text[..i].to_string(),
        None => text.to_string(),
    }
}

/// [(page, text)] of a file, page 0 when it has no pages, and its count of pages without text.
fn file_pages(mut f: Source, suffix: &str) -> Result<(Vec<(i64, String)>, i64), String> {
    if suffix == ".pdf" {
        let pages: Vec<(i64, String)> = without_running_lines(pdf_pages(f, 1, None)?.1)
            .into_iter()
            .enumerate()
            .map(|(n, t)| (n as i64 + 1, t.py_trim().to_string()))
            .collect();
        let empty = pages.iter().filter(|(_, t)| t.is_empty()).count() as i64;
        return Ok((
            pages.into_iter().filter(|(_, t)| !t.is_empty()).collect(),
            empty,
        ));
    }
    let text = if DOCUMENT_SUFFIXES.contains(&suffix) {
        take_chars(&document_text(f, suffix)?, MAX_FILE_CHARS)
    } else {
        let mut data = Vec::new();
        f.by_ref()
            .take(MAX_FILE_CHARS as u64 * 4)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&data).into_owned();
        // Indexed by their text alone; read_file still shows them as they are.
        let text = match suffix {
            ".html" | ".htm" => html_text(&text, false),
            ".xml" => html_text(&text, true),
            _ => text,
        };
        take_chars(&text, MAX_FILE_CHARS)
    };
    Ok((
        if text.py_trim().is_empty() {
            vec![]
        } else {
            vec![(0, text)]
        },
        0,
    ))
}

/// Why a file was not done. Only `Embed` and `Db` can stop the run.
enum FileError {
    Embed(EmbedError),
    Nextcloud(String),
    File(String),
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for FileError {
    fn from(e: rusqlite::Error) -> Self {
        FileError::Db(e)
    }
}

impl From<Error> for FileError {
    fn from(e: Error) -> Self {
        match e {
            Error::Nextcloud { message, .. } => FileError::Nextcloud(message),
            Error::User(m) => FileError::File(format!("ValueError: {m}")),
            Error::NotFound => FileError::File("FileNotFoundError: not found.".into()),
            Error::Io(e) => FileError::File(format!("OSError: {}", folders::io_message(&e))),
        }
    }
}

/// Why a run stopped for a user: the embedding server, or anything else.
pub enum RunError {
    Embed(EmbedError),
    Other(String),
}

impl From<rusqlite::Error> for RunError {
    fn from(e: rusqlite::Error) -> Self {
        RunError::Other(format!("sqlite: {e}"))
    }
}

/// What a run does with one file: its outcome, the file left as it was on error.
struct Run<'a> {
    db: &'a Connection,
    embedder: &'a Embedder,
    family: &'a str,
}

impl Run<'_> {
    /// Store one file's passages and vectors; returns its status.
    fn index_file(&self, item: &Item, f: Source, digest: &str) -> Result<&'static str, FileError> {
        let shown = &item.shown;
        let ext = suffix(shown);
        let title = shown.rsplit('/').next().unwrap_or(shown);
        let (mut status, mut empty, mut error, mut rows) = ("indexed", 0, None, Vec::new());
        if image_format(&ext).is_some() {
            status = "needs_ocr";
        } else {
            match file_pages(f, &ext) {
                Ok((pages, e)) => {
                    empty = e;
                    rows = pages
                        .iter()
                        .flat_map(|(page, whole)| {
                            passages(whole, title).into_iter().map(|t| (*page, t))
                        })
                        .collect();
                }
                Err(e) => {
                    status = "failed";
                    error = Some(take_chars(&format!("ValueError: {e}"), 300));
                }
            }
            if status == "indexed" && rows.is_empty() {
                status = if ext == ".pdf" { "needs_ocr" } else { "empty" };
            }
        }
        let texts: Vec<String> = rows
            .iter()
            .map(|(_, t)| embed::passage(self.family, title, t))
            .collect();
        let vectors = if texts.is_empty() {
            vec![]
        } else {
            self.embedder.embed(&texts).map_err(FileError::Embed)?
        };
        self.db.execute(
            "INSERT INTO files (path, size, mtime_ns, ctime_ns, sha256, status, empty_pages, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![shown, item.size, item.mtime_ns, item.ctime_ns, digest, status, empty, error],
        )?;
        let id = self.db.last_insert_rowid();
        let mut insert = self
            .db
            .prepare("INSERT INTO passages (file, page, text, vector) VALUES (?, ?, ?, ?)")?;
        for ((page, text), vector) in rows.iter().zip(&vectors) {
            insert.execute(params![id, page, text, embed::to_blob(vector)])?;
        }
        Ok(status)
    }
}

/// A row of the files table, as the run compares it.
struct Known {
    id: i64,
    size: i64,
    mtime_ns: i64,
    ctime_ns: i64,
    sha256: Option<String>,
    error: Option<String>,
}

/// ({shown path: Item} of every file the index can use, {folders and labels
/// Nextcloud did not list}). Hidden names, links out and what the filters
/// leave out are not used.
fn walk(tree: &mut Tree) -> (BTreeMap<String, Item>, BTreeSet<String>) {
    let mut found = BTreeMap::new();
    let mut unreachable = BTreeSet::new();
    let labels: Vec<String> = tree.roots.iter().map(|(l, _)| l.clone()).collect();
    for label in labels {
        let items = tree.item(&label, &[]).and_then(|top| match top {
            Some(top) if top.is_dir => tree.walk(&top),
            _ => Ok(vec![]),
        });
        match items {
            Ok(items) => found.extend(
                items
                    .into_iter()
                    .filter(|i| !i.is_dir && usable(&suffix(&i.shown)))
                    .map(|i| (i.shown.clone(), i)),
            ),
            Err(e) => {
                eprintln!("{label}: {e} What the index holds of it stays.");
                unreachable.insert(label);
            }
        }
    }
    for shown in &tree.unlisted {
        eprintln!("{shown}: Nextcloud did not list it. What the index holds of it stays.");
        unreachable.insert(shown.clone());
    }
    (found, unreachable)
}

fn usable(ext: &str) -> bool {
    TEXT_SUFFIXES.contains(&ext)
        || DOCUMENT_SUFFIXES.contains(&ext)
        || image_format(ext).is_some()
        || ext == ".pdf"
}

pub type Counts = BTreeMap<String, usize>;

/// Bring one user's index in line with their folders. Returns ({outcome:
/// count}, {labels Nextcloud did not list}). Stops at the quiet hours with
/// what it has done published; counts then holds "paused".
fn index_user(
    db: &Connection,
    own: &UserFolders,
    nextcloud: Option<&Nextcloud>,
    run: &Run,
    model: &str,
    base: &Path,
    quiet: &str,
) -> Result<(Counts, BTreeSet<String>), RunError> {
    let meta: HashMap<String, String> = db
        .prepare("SELECT key, value FROM meta")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    if meta.get("model").map(String::as_str) != Some(model) {
        db.execute_batch("DELETE FROM passages; DELETE FROM files;")?;
        db.execute("INSERT OR REPLACE INTO meta VALUES ('model', ?)", [model])?;
    }
    let stored: BTreeMap<String, String> = meta
        .get("folders")
        .and_then(|f| serde_json::from_str(f).ok())
        .unwrap_or_default();
    let roots: BTreeMap<&str, &str> = own
        .roots
        .iter()
        .map(|(l, r)| (l.as_str(), r.as_str()))
        .collect();
    for (label, path) in &stored {
        if roots.get(label.as_str()) != Some(&path.as_str()) {
            let ids: Vec<i64> = db
                .prepare("SELECT id FROM files WHERE substr(path, 1, ?) = ?")?
                .query_map(
                    params![label.chars().count() as i64 + 1, format!("{label}/")],
                    |r| r.get(0),
                )?
                .collect::<Result<_, _>>()?;
            for id in ids {
                drop_file(db, id)?;
            }
        }
    }
    db.execute(
        "INSERT OR REPLACE INTO meta VALUES ('folders', ?)",
        [serde_json::to_string(&roots).unwrap_or_default()],
    )?;

    let mut tree = Tree::new(&own.roots, &own.rules, nextcloud);
    tree.cache = db
        .prepare("SELECT path, listing FROM nextcloud_folders")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(Result::ok)
        .filter_map(|(path, listing)| {
            let v: Value = serde_json::from_str(&listing).ok()?;
            let entries = v[1]
                .as_array()?
                .iter()
                .map(Entry::from_json)
                .collect::<Option<Vec<_>>>()?;
            Some((path, (v[0].as_str().unwrap_or("").to_string(), entries)))
        })
        .collect();
    let (found, mut unreachable) = walk(&mut tree);
    // What Nextcloud did not list is kept as it was, neither dropped nor taken as moved.
    let known: HashMap<String, Known> = db
        .prepare("SELECT path, id, size, mtime_ns, ctime_ns, sha256, error FROM files")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Known {
                    id: r.get(1)?,
                    size: r.get::<_, Option<i64>>(2)?.unwrap_or(-1),
                    mtime_ns: r.get::<_, Option<i64>>(3)?.unwrap_or(-1),
                    ctime_ns: r.get::<_, Option<i64>>(4)?.unwrap_or(-1),
                    sha256: r.get(5)?,
                    error: r.get(6)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(path, _)| {
            !unreachable
                .iter()
                .any(|u| path.starts_with(&format!("{u}/")))
        })
        .collect();
    let mut gone: HashMap<String, i64> = known
        .iter()
        .filter(|(path, _)| !found.contains_key(*path))
        .filter_map(|(_, k)| k.sha256.clone().map(|s| (s, k.id)))
        .collect();
    let mut reused: HashSet<i64> = HashSet::new();
    let mut counts = Counts::new();
    let mut last_publish = Instant::now();
    for (shown, item) in &found {
        // The rest waits for the next run, which skips what this one did.
        if quiet_now(quiet) {
            counts.insert("paused".into(), 1);
            publish(db, base)?;
            return Ok((counts, unreachable));
        }
        let row = known.get(shown);
        db.execute_batch("BEGIN")?;
        let outcome = match one_file(db, &tree, run, item, row, &mut gone, &mut reused) {
            Ok(outcome) => outcome,
            Err(FileError::Embed(EmbedError::Status(code)))
                if !SERVER_REFUSED.contains(&code) && !run.embedder.is_remote() =>
            {
                failed(db, item, row, &EmbedError::Status(code).to_string())?
            }
            Err(FileError::Embed(e @ EmbedError::Shape(_))) => {
                failed(db, item, row, &e.to_string())?
            }
            Err(FileError::Embed(e)) => return Err(RunError::Embed(e)),
            Err(FileError::Db(e)) => return Err(e.into()),
            Err(FileError::Nextcloud(message)) => {
                // Not the file's fault: kept as it was, and tried again next run.
                db.execute_batch("ROLLBACK; BEGIN")?;
                eprintln!("{shown}: {message}");
                unreachable.insert(shown.split('/').next().unwrap_or("").to_string());
                "skipped"
            }
            Err(FileError::File(message)) => failed(db, item, row, &message)?,
        };
        db.execute_batch("COMMIT")?;
        *counts.entry(outcome.into()).or_default() += 1;
        if last_publish.elapsed().as_secs() > PUBLISH_EVERY_S {
            publish(db, base)?;
            last_publish = Instant::now();
        }
    }
    for (path, row) in &known {
        if !found.contains_key(path) && !reused.contains(&row.id) {
            drop_file(db, row.id)?;
            *counts.entry("dropped".into()).or_default() += 1;
        }
    }
    // The listings of this run only, so a folder gone or filtered out is not kept.
    if unreachable.is_empty() {
        db.execute("DELETE FROM nextcloud_folders", [])?;
    }
    for (path, (etag, entries)) in &tree.listed {
        let listing = json!([etag, entries.iter().map(Entry::to_json).collect::<Vec<_>>()]);
        db.execute(
            "INSERT OR REPLACE INTO nextcloud_folders VALUES (?, ?)",
            params![path, listing.to_string()],
        )?;
    }
    publish(db, base)?;
    Ok((counts, unreachable))
}

/// Kept as failed with its dates, so it is retried once it changes.
fn failed(
    db: &Connection,
    item: &Item,
    row: Option<&Known>,
    message: &str,
) -> rusqlite::Result<&'static str> {
    db.execute_batch("ROLLBACK; BEGIN")?;
    if let Some(row) = row {
        drop_file(db, row.id)?;
    }
    db.execute(
        "INSERT INTO files (path, size, mtime_ns, ctime_ns, status, error) VALUES (?, ?, ?, ?, 'failed', ?)",
        params![item.shown, item.size, item.mtime_ns, item.ctime_ns, take_chars(message, 300)],
    )?;
    Ok("failed")
}

fn one_file(
    db: &Connection,
    tree: &Tree,
    run: &Run,
    item: &Item,
    row: Option<&Known>,
    gone: &mut HashMap<String, i64>,
    reused: &mut HashSet<i64>,
) -> Result<&'static str, FileError> {
    // A file failed by the embedding server, not by its content, is tried again.
    if let Some(row) = row
        && (row.size, row.mtime_ns, row.ctime_ns) == (item.size, item.mtime_ns, item.ctime_ns)
        && !row
            .error
            .as_deref()
            .unwrap_or("")
            .starts_with("HTTPStatusError")
    {
        return Ok("unchanged");
    }
    let mut f = tree.open(item, MAX_FETCH_BYTES)?;
    let digest = sha256(&mut f)
        .map_err(|e| FileError::File(format!("OSError: {}", folders::io_message(&e))))?;
    let moved = if row.is_none() {
        gone.remove(&digest)
    } else {
        None
    };
    if let Some(id) = moved {
        reused.insert(id);
    }
    let same = row.is_some_and(|r| r.sha256.as_deref() == Some(digest.as_str()));
    if moved.is_some() || same {
        let id = moved.or(row.map(|r| r.id)).unwrap_or_default();
        db.execute(
            "UPDATE files SET path = ?, size = ?, mtime_ns = ?, ctime_ns = ? WHERE id = ?",
            params![item.shown, item.size, item.mtime_ns, item.ctime_ns, id],
        )?;
        return Ok(if moved.is_some() {
            "moved"
        } else {
            "unchanged"
        });
    }
    if let Some(row) = row {
        drop_file(db, row.id)?;
    }
    run.index_file(item, f, &digest)
}

/// The lowest cosine between the Pi's and Whoopy's vectors of the check
/// passages, or why Whoopy gave none. An error from the Pi is the run's.
pub fn desktop_check(
    local: &Embedder,
    remote: &Embedder,
    family: &str,
) -> Result<Result<f32, String>, EmbedError> {
    let texts: Vec<String> = CHECK_PASSAGES
        .iter()
        .map(|t| embed::passage(family, "check.md", t))
        .collect();
    let here = local.embed(&texts)?;
    Ok(match remote.embed(&texts) {
        Ok(there) => Ok(here
            .iter()
            .zip(&there)
            .map(|(a, b)| embed::dot(a, b))
            .fold(f32::INFINITY, f32::min)),
        Err(EmbedError::Status(code)) => Err(format!(
            "Whoopy answered {code} to the check passages: embedded on this server."
        )),
        Err(e) => {
            let kind = match e {
                EmbedError::Transport(_) => "TransportError",
                _ => "ValueError",
            };
            Err(format!(
                "Whoopy did not embed the check passages ({kind}): embedded on this server."
            ))
        }
    })
}

/// The run's settings, from the environment.
pub struct Settings {
    pub index_dir: std::path::PathBuf,
    pub folders_file: String,
    pub embed_url: String,
    pub embed_model: String,
    pub family: String,
    pub quiet: String,
    pub remote_url: String,
    pub remote_model: String,
    pub remote_key: String,
}

impl Settings {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let need = |k: &str| std::env::var(k).map_err(|_| format!("{k} is not set."));
        let family = var("DOCS_EMBED_NAME");
        if embed::query_prefix(&family).is_none() {
            return Err(format!(
                "DOCS_EMBED_NAME: no model family {}.",
                folders::py_repr(&family)
            ));
        }
        Ok(Settings {
            index_dir: need("DOCS_INDEX_DIR")?.into(),
            folders_file: need("DOCS_FOLDERS_FILE")?,
            embed_url: need("DOCS_EMBED_URL")?,
            embed_model: need("DOCS_EMBED_MODEL")?,
            family,
            quiet: var("DOCS_QUIET_HOURS"),
            remote_url: var("DOCS_REMOTE_URL"),
            remote_model: var("DOCS_REMOTE_MODEL"),
            remote_key: var("DOCS_REMOTE_KEY"),
        })
    }

    pub fn remote(&self) -> Embedder {
        let url = if self.remote_url.is_empty() {
            "http://127.0.0.1:4000"
        } else {
            &self.remote_url
        };
        Embedder::remote(url, &self.remote_model, &self.remote_key)
    }
}

/// One index run for every user. An error is the run's failure, said once.
pub fn main(s: &Settings) -> Result<(), String> {
    // Taken first, so a run that fails to start leaves no "remote" for the timer.
    let mut quiet = s.quiet.clone();
    let run_now = s.index_dir.join("run-now");
    let wanted = match std::fs::read(&run_now) {
        Ok(data) => {
            let _ = std::fs::remove_file(&run_now);
            quiet.clear();
            Some(
                take_chars(&String::from_utf8_lossy(&data), 16)
                    .trim()
                    .to_string(),
            )
        }
        Err(_) => None,
    };
    let users = folders::load_folders(&s.folders_file)?;
    let nextcloud = Nextcloud::from_env()?;
    needs_nextcloud(&users, nextcloud.as_ref())?;
    // A password refused before is not tried again: each try counts against loopback.
    if let Some(nc) = &nextcloud
        && users.keys().any(|u| {
            read_status(&s.index_dir.join(u)).get("nextcloud_refused") == Some(&json!(nc.mark))
        })
    {
        nc.refused.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if quiet_now(&quiet) {
        // A timer catching up after a stop: the next run outside them will do.
        println!("quiet hours {quiet}: no run now.");
        return Ok(());
    }
    let model = format!("{}/passages-{PASSAGES_VERSION}", s.embed_model);
    let local = Embedder::local(&s.embed_url);
    let remote = s.remote();
    if !local.wait_ready() {
        return Err("the embedding server does not answer.".into());
    }
    let mut use_remote = false;
    let mut note: Option<String> = None;
    if wanted.as_deref() == Some("remote") {
        if s.remote_key.is_empty() || s.remote_model.is_empty() {
            note = Some(
                "No Whoopy key or model is saved in the panel: embedded on this server.".into(),
            );
        } else {
            match desktop_check(&local, &remote, &s.family).map_err(|e| e.to_string())? {
                Ok(lowest) if lowest >= MIN_COSINE => use_remote = true,
                Ok(lowest) => {
                    note = Some(format!(
                        "The desktop's vectors differ from this server's (cosine {lowest:.4}, under {MIN_COSINE}): embedded on this server."
                    ))
                }
                Err(why) => note = Some(why),
            }
        }
    }
    if use_remote {
        println!(
            "Embedding on the desktop through Whoopy, {}.",
            s.remote_model
        );
    } else if let Some(note) = &note {
        println!("{note}");
    }
    let mut broken = Vec::new();
    for (user, own) in &users {
        let base = s.index_dir.join(user);
        let db = connect(&base.with_extension("work.sqlite"), false)
            .map_err(|e| format!("{user}: {e}"))?;
        write_status(
            &base,
            json!({"started": now(), "ended": null, "result": "running", "run": {}, "error": null,
                   "embedded_on": [if use_remote { "desktop" } else { "pi" }], "remote_note": note}),
        );
        let mut result = {
            let run = Run {
                db: &db,
                embedder: if use_remote { &remote } else { &local },
                family: &s.family,
            };
            index_user(&db, own, nextcloud.as_ref(), &run, &model, &base, &quiet)
        };
        if use_remote && let Err(RunError::Embed(e)) = &result {
            // Each file done is committed: the Pi carries on from there.
            if !db.is_autocommit() {
                let _ = db.execute_batch("ROLLBACK");
            }
            let kind = match e {
                EmbedError::Transport(_) => "TransportError",
                _ => "HTTPStatusError",
            };
            let why = format!("Whoopy stopped answering ({kind}): the run went on on this server.");
            eprintln!("{user}: {why}");
            write_status(
                &base,
                json!({"embedded_on": ["desktop", "pi"], "remote_note": why}),
            );
            note = Some(why);
            use_remote = false;
            let run = Run {
                db: &db,
                embedder: &local,
                family: &s.family,
            };
            result = index_user(&db, own, nextcloud.as_ref(), &run, &model, &base, &quiet);
        }
        match result {
            Ok((mut counts, unreachable)) => {
                let paused = counts.remove("paused").is_some();
                let said = counts
                    .iter()
                    .map(|(k, n)| format!("{n} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let said = if said.is_empty() {
                    "no files".to_string()
                } else {
                    said
                };
                let pause = if paused {
                    format!(", paused for the quiet hours {quiet}")
                } else {
                    String::new()
                };
                println!("{user}: {said}{pause}");
                let refused = nextcloud
                    .as_ref()
                    .filter(|nc| nc.is_refused())
                    .map(|nc| nc.mark.clone());
                let names = unreachable.iter().cloned().collect::<Vec<_>>().join(", ");
                write_status(
                    &base,
                    json!({"ended": now(), "nextcloud_refused": refused,
                           "result": if !unreachable.is_empty() { "failed" } else if paused { "paused" } else { "done" },
                           "run": counts,
                           "error": if unreachable.is_empty() { Value::Null } else { json!(format!("Nextcloud did not list {names}.")) }}),
                );
                if !unreachable.is_empty() {
                    broken.push(user.clone());
                }
            }
            Err(RunError::Embed(EmbedError::Transport(e))) => {
                write_status(
                    &base,
                    json!({"ended": now(), "result": "failed", "error": "the embedding server stopped answering."}),
                );
                return Err(format!("the embedding server stopped answering: {e}"));
            }
            Err(e) => {
                // One user's broken index must not stop the others.
                let message = match e {
                    RunError::Embed(e) => e.to_string(),
                    RunError::Other(m) => m,
                };
                eprintln!("{user}: index failed: {message}");
                write_status(
                    &base,
                    json!({"ended": now(), "result": "failed", "error": take_chars(&message, 300)}),
                );
                broken.push(user.clone());
            }
        }
    }
    // A user taken out of the settings takes their index along.
    for entry in std::fs::read_dir(&s.index_dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_file && !users.contains_key(name.split('.').next().unwrap_or("")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    if !broken.is_empty() {
        return Err(format!("index failed for {}.", broken.join(", ")));
    }
    Ok(())
}

/// Refuses a folder in Nextcloud when no account is set.
pub fn needs_nextcloud(
    users: &BTreeMap<String, UserFolders>,
    nextcloud: Option<&Nextcloud>,
) -> Result<(), String> {
    let cloud = users
        .values()
        .flat_map(|u| &u.roots)
        .any(|(_, r)| r.starts_with(folders::NEXTCLOUD));
    if nextcloud.is_none() && cloud {
        return Err("A folder is in Nextcloud, and DOCS_NEXTCLOUD_URL is not set.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_stops_when_the_quiet_hours_begin() {
        use chrono::Timelike;
        let dir = std::env::temp_dir().join(format!("cheruby-docs-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("notes/a.md"), "pain, lait").unwrap();
        let own = UserFolders {
            roots: vec![(
                "Notes".into(),
                dir.join("notes").to_string_lossy().into_owned(),
            )],
            ..Default::default()
        };
        let hour = chrono::Local::now().hour();
        let quiet = format!("{hour:02}-{:02}", (hour + 1) % 24);
        let base = dir.join("alice");
        let db = connect(&base.with_extension("work.sqlite"), false).unwrap();
        // Nothing listens there: a run that embedded anything would fail.
        let embedder = Embedder::local("http://127.0.0.1:9");
        let run = Run {
            db: &db,
            embedder: &embedder,
            family: "embeddinggemma",
        };
        let Ok((counts, _)) = index_user(&db, &own, None, &run, "m/passages-3", &base, &quiet)
        else {
            panic!("the run failed");
        };
        assert_eq!(counts, Counts::from([("paused".to_string(), 1)]));
        assert!(base.with_extension("sqlite").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn quiet_hours_wrap() {
        assert!(quiet_at("01-06", 1) && quiet_at("01-06", 5) && !quiet_at("01-06", 6));
        assert!(quiet_at("22-02", 23) && quiet_at("22-02", 1) && !quiet_at("22-02", 2));
        assert!(!quiet_at("", 3));
    }
}
