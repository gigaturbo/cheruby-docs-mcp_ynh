"""Search index: each user's documents cut into passages, with their vectors.

Run by cheruby-docs-mcp-index.service, hourly. Settings come from its
root-600 env file, which holds no token:

    DOCS_FOLDERS_<user>     as for the tool server
    DOCS_INDEX_DIR          where each user's index is written
    DOCS_EMBED_URL          the embedding server, e.g. http://127.0.0.1:8771
    DOCS_EMBED_MODEL        the model's identity; when it changes, everything is redone

Each run walks the folders and redoes only the files whose size or dates
changed; a moved file keeps its passages, a deleted one loses them, and so do
the files of a folder taken away or pointed elsewhere. A file that cannot be
read is marked failed and retried once it changes; the run goes on. Text
comes from the files' own text: scans and images are marked as waiting for
OCR and hold no passage. Paths are stored as the tools show them
(Documents/...), never as real paths.

The indexer works on <user>.work.sqlite and publishes a finished copy as
<user>.sqlite every ten minutes and at the end, so the tool server, which only
reads, never sees a write half done. It imports connect(), folders_key() and
QUERY_PROMPT from here.
"""

import hashlib
import json
import os
import sqlite3
import sys
import time
import urllib.parse

import httpx2
import numpy

from docs_files import IMAGE_FORMATS, OFFICE_MEMBERS, TEXT_SUFFIXES, hidden, inside, load_folders, office_text, open_pdf

# EmbeddingGemma is trained with these prefixes; without them results get worse.
QUERY_PROMPT = "task: search result | query: "
DOC_PROMPT = "title: {title} | text: {text}"

# About 400 tokens, well under the model's 2048 even at one token a character.
PASSAGE_CHARS = 1600
OVERLAP_CHARS = 200
MAX_FILE_CHARS = 300_000
BATCH = 8
PUBLISH_EVERY_S = 600

SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, size INTEGER, mtime_ns INTEGER,
    ctime_ns INTEGER, sha256 TEXT, status TEXT, empty_pages INTEGER DEFAULT 0, error TEXT);
CREATE TABLE IF NOT EXISTS passages (
    id INTEGER PRIMARY KEY, file INTEGER NOT NULL, page INTEGER, text TEXT, vector BLOB);
CREATE INDEX IF NOT EXISTS passages_file ON passages (file);
CREATE VIRTUAL TABLE IF NOT EXISTS passages_fts USING fts5(
    text, content='passages', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
CREATE TRIGGER IF NOT EXISTS passages_ai AFTER INSERT ON passages BEGIN
    INSERT INTO passages_fts (rowid, text) VALUES (new.id, new.text); END;
CREATE TRIGGER IF NOT EXISTS passages_ad AFTER DELETE ON passages BEGIN
    INSERT INTO passages_fts (passages_fts, rowid, text) VALUES ('delete', old.id, old.text); END;
"""


def connect(path, read_only=False):
    """The index at path. Read-only opens never create or change it."""
    if read_only:
        return sqlite3.connect(f"file:{urllib.parse.quote(path)}?mode=ro", uri=True, timeout=10)
    db = sqlite3.connect(path, timeout=10)
    db.executescript(SCHEMA)
    return db


def folders_key(folders):
    """The folders as stored in the index, to tell whether it still matches the settings."""
    return json.dumps(folders, sort_keys=True)


def passages(text):
    """Text cut into passages of about PASSAGE_CHARS, overlapping, cut at a sentence or a word."""
    text = " ".join(text.split())
    out, start = [], 0
    while start < len(text):
        end = start + PASSAGE_CHARS
        if end >= len(text):
            out.append(text[start:])
            break
        cut = text.rfind(". ", start + PASSAGE_CHARS // 2, end)
        if cut < 0:
            cut = text.rfind(" ", start + PASSAGE_CHARS // 2, end)
        cut = end if cut < 0 else cut + 1
        out.append(text[start:cut].strip())
        space = text.find(" ", cut - OVERLAP_CHARS, cut)
        start = cut - OVERLAP_CHARS if space < 0 else space + 1
    return [p for p in out if p]


def file_pages(real):
    """[(page, text)] of a file, page 0 when it has no pages, and its count of pages without text."""
    suffix = os.path.splitext(real)[1].lower()
    if suffix == ".pdf":
        pages = [(n, (page.extract_text() or "").strip()) for n, page in enumerate(open_pdf(real).pages, 1)]
        return [(n, t) for n, t in pages if t], sum(1 for _, t in pages if not t)
    if suffix in OFFICE_MEMBERS:
        text = office_text(real)
    else:
        with open(real, encoding="utf-8", errors="replace") as f:
            text = f.read(MAX_FILE_CHARS)
    return ([(0, text)] if text.strip() else []), 0


def embed(client, texts):
    """Unit vectors for texts, in order, as float32 rows."""
    rows = []
    for i in range(0, len(texts), BATCH):
        resp = client.post("/v1/embeddings", json={"input": texts[i:i + BATCH]})
        resp.raise_for_status()
        rows += [d["embedding"] for d in sorted(resp.json()["data"], key=lambda d: d["index"])]
    matrix = numpy.asarray(rows, dtype=numpy.float32)
    return matrix / numpy.linalg.norm(matrix, axis=1, keepdims=True)


def sha256(real):
    h = hashlib.sha256()
    with open(real, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def walk(folders):
    """{shown path: real path} of every file the index can use, hidden names and links out left out."""
    found = {}
    usable = TEXT_SUFFIXES | OFFICE_MEMBERS.keys() | IMAGE_FORMATS.keys() | {".pdf"}
    for label, root in folders.items():
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if not hidden(d)]
            rel = os.path.relpath(dirpath, root)
            for name in filenames:
                real = os.path.join(dirpath, name)
                if hidden(name) or os.path.splitext(name)[1].lower() not in usable:
                    continue
                if not inside(os.path.realpath(real), root) or not os.path.isfile(real):
                    continue
                found["/".join(p for p in (label, "" if rel == "." else rel, name) if p)] = real
    return found


def drop(db, file_id):
    db.execute("DELETE FROM passages WHERE file = ?", (file_id,))
    db.execute("DELETE FROM files WHERE id = ?", (file_id,))


def index_file(db, client, shown, real, st, digest):
    """Store one file's passages and vectors; returns its status. Embedding errors propagate."""
    suffix = os.path.splitext(real)[1].lower()
    status, empty, error, rows = "indexed", 0, None, []
    if suffix in IMAGE_FORMATS:
        status = "needs_ocr"
    else:
        try:
            pages, empty = file_pages(real)
        except Exception as e:  # any reader error: the file is marked, the run goes on
            pages, status, error = [], "failed", f"{type(e).__name__}: {e}"[:300]
        rows = [(page, text) for page, whole in pages for text in passages(whole)]
        if status == "indexed" and not rows:
            status = "needs_ocr" if suffix == ".pdf" else "empty"
    vectors = embed(client, [DOC_PROMPT.format(title=os.path.basename(shown), text=t) for _, t in rows]) if rows else []
    cur = db.execute("INSERT INTO files (path, size, mtime_ns, ctime_ns, sha256, status, empty_pages, error) "
                     "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                     (shown, st.st_size, st.st_mtime_ns, st.st_ctime_ns, digest, status, empty, error))
    db.executemany("INSERT INTO passages (file, page, text, vector) VALUES (?, ?, ?, ?)",
                   [(cur.lastrowid, page, text, vec.tobytes()) for (page, text), vec in zip(rows, vectors)])
    return status


def publish(db, path):
    """Copy the working index to path in one step, stamped with the time."""
    db.execute("INSERT OR REPLACE INTO meta VALUES ('updated', ?)", (time.strftime("%Y-%m-%d %H:%M"),))
    db.commit()
    dest = sqlite3.connect(path + ".tmp")
    db.backup(dest)
    dest.close()
    os.replace(path + ".tmp", path)


def index_user(db, folders, client, model, published):
    """Bring one user's index in line with their folders. Returns {outcome: count}."""
    meta = dict(db.execute("SELECT key, value FROM meta"))
    if meta.get("model") != model:
        db.executescript("DELETE FROM passages; DELETE FROM files;")
        db.execute("INSERT OR REPLACE INTO meta VALUES ('model', ?)", (model,))
    for label, path in json.loads(meta.get("folders", "{}")).items():
        if folders.get(label) != path:
            for (file_id,) in db.execute("SELECT id FROM files WHERE substr(path, 1, ?) = ?",
                                         (len(label) + 1, label + "/")).fetchall():
                drop(db, file_id)
    db.execute("INSERT OR REPLACE INTO meta VALUES ('folders', ?)", (folders_key(folders),))
    db.commit()

    found = walk(folders)
    known = {r[0]: r[1:] for r in db.execute("SELECT path, id, size, mtime_ns, ctime_ns, sha256 FROM files")}
    gone = {row[4]: row[0] for path, row in known.items() if path not in found}
    reused, counts, last_publish = set(), {}, time.monotonic()
    for shown, real in sorted(found.items()):
        row = known.get(shown)
        st = None
        try:
            st = os.stat(real)
            if row and row[1:4] == (st.st_size, st.st_mtime_ns, st.st_ctime_ns):
                outcome = "unchanged"
            else:
                digest = sha256(real)
                moved = gone.pop(digest, None) if row is None else None
                reused.add(moved)
                if moved is not None or (row and row[4] == digest):
                    db.execute("UPDATE files SET path = ?, size = ?, mtime_ns = ?, ctime_ns = ? WHERE id = ?",
                               (shown, st.st_size, st.st_mtime_ns, st.st_ctime_ns,
                                moved if moved is not None else row[0]))
                    outcome = "moved" if moved is not None else "unchanged"
                else:
                    if row:
                        drop(db, row[0])
                    outcome = index_file(db, client, shown, real, st, digest)
        except httpx2.TransportError:
            raise
        except (OSError, ValueError, httpx2.HTTPStatusError) as e:
            # Kept as failed with its dates, so it is retried once it changes.
            db.rollback()
            if row:
                drop(db, row[0])
            if st:
                db.execute("INSERT INTO files (path, size, mtime_ns, ctime_ns, status, error) "
                           "VALUES (?, ?, ?, ?, 'failed', ?)", (shown, st.st_size, st.st_mtime_ns,
                                                                st.st_ctime_ns, f"{type(e).__name__}: {e}"[:300]))
            outcome = "failed"
        db.commit()
        counts[outcome] = counts.get(outcome, 0) + 1
        if time.monotonic() - last_publish > PUBLISH_EVERY_S:
            publish(db, published)
            last_publish = time.monotonic()
    for path, row in known.items():
        if path not in found and row[0] not in reused:
            drop(db, row[0])
            counts["dropped"] = counts.get("dropped", 0) + 1
    publish(db, published)
    return counts


def wait_ready(client):
    """The embedding server loads its model at start; wait up to two minutes."""
    for _ in range(60):
        try:
            if client.get("/health").status_code == 200:
                return
        except httpx2.TransportError:
            pass
        time.sleep(2)
    sys.exit("the embedding server does not answer.")


def main():
    users = load_folders(os.environ)
    index_dir = os.environ["DOCS_INDEX_DIR"]
    broken = []
    with httpx2.Client(base_url=os.environ["DOCS_EMBED_URL"], timeout=300) as client:
        wait_ready(client)
        for user, folders in sorted(users.items()):
            base = os.path.join(index_dir, user)
            db = connect(base + ".work.sqlite")
            try:
                counts = index_user(db, folders, client, os.environ["DOCS_EMBED_MODEL"], base + ".sqlite")
                print(f"{user}: " + (", ".join(f"{n} {k}" for k, n in sorted(counts.items())) or "no files"), flush=True)
            except httpx2.TransportError:
                raise
            except Exception as e:  # one user's broken index must not stop the others
                print(f"{user}: index failed: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
                broken.append(user)
            finally:
                db.close()
    # A user taken out of the settings takes their index along.
    for name in os.listdir(index_dir):
        if name.split(".")[0] not in users:
            os.remove(os.path.join(index_dir, name))
    if broken:
        sys.exit(f"index failed for {', '.join(broken)}.")


if __name__ == "__main__":
    main()
