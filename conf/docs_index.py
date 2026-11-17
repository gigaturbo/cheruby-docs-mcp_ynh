"""Search index: each user's documents cut into passages, with their vectors.

Run by cheruby-docs-mcp-index.service, hourly. Settings come from its
root-600 env file, which holds no MCP token:

    DOCS_FOLDERS_FILE       as for the tool server, and DOCS_NEXTCLOUD_* when a folder is in Nextcloud
    DOCS_INDEX_DIR          where each user's index is written
    DOCS_EMBED_URL          the embedding server, e.g. http://127.0.0.1:8771
    DOCS_EMBED_MODEL        the model's identity; when it changes, everything is redone
    DOCS_QUIET_HOURS        optional, such as 01-06: no run in these hours, and one
                            still going stops when they begin. A file run-now in
                            the index folder, left by the panel's Run now, lifts
                            this for one run.

Each run walks the folders and redoes only the files whose size or dates
changed; a moved file keeps its passages, a deleted one loses them, and so do
the files of a folder taken away or pointed elsewhere, and those its filters
now leave out. A file that cannot be
read is marked failed and retried once it changes; the run goes on. A
Nextcloud folder is listed only where its ETag changed since the last run;
when Nextcloud does not answer, what the index holds of it stays, and the
run ends as failed. Text
comes from the files' own text: scans and images are marked as waiting for
OCR and hold no passage. Paths are stored as the tools show them
(Documents/...), never as real paths.

The indexer works on <user>.work.sqlite and publishes a finished copy as
<user>.sqlite every ten minutes and at the end, so the tool server, which only
reads, never sees a write half done. It imports connect() and
QUERY_PROMPT from here. Deleting the working copy makes the next run start
from zero; search goes on with the published one until its first publish.

<user>.status.json, for the config panel: when the last run started and
ended, how it ended, what it did, and the index's files by status.
"""

import hashlib
import json
import os
import re
import sqlite3
import sys
import time
import urllib.parse

import httpx2
import numpy

from docs_files import IMAGE_FORMATS, TEXT_SUFFIXES, Folders, Nextcloud, NextcloudError, load_folders, needs_nextcloud
from docs_text import DOCUMENT_SUFFIXES, document_text, open_pdf

# EmbeddingGemma is trained with these prefixes; without them results get worse.
QUERY_PROMPT = "task: search result | query: "
DOC_PROMPT = "title: {title} | text: {text}"

# About 250 tokens: a section of a note stays its own passage instead of an
# average of its neighbours. Well under the model's 2048 tokens.
PASSAGE_CHARS = 900
OVERLAP_CHARS = 150
# Joins the model in meta: a change to how passages are cut redoes the index.
PASSAGES_VERSION = 2
HEADING = re.compile(r"^(#{1,6})\s+(.+?)[\s#]*$")
MAX_FILE_CHARS = 300_000
MAX_FETCH_BYTES = 64_000_000
BATCH = 8
MIN_EMBED_CHARS = 200
PUBLISH_EVERY_S = 600

SCHEMA = """
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


def split_long(text):
    """A paragraph of folded text cut into pieces of about PASSAGE_CHARS,
    overlapping, at a sentence or else a word."""
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


def sections(text):
    """[(headings above it, its text)] of a text: a Markdown heading line opens
    a section, outside code fences, whose "# comment" lines are not headings."""
    out, trail, lines, fenced = [], [], [], False
    for line in text.splitlines():
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
        heading = None if fenced else HEADING.match(line)
        if heading:
            out.append((trail, "\n".join(lines)))
            level = len(heading[1])
            trail, lines = [h for h in trail if h[0] < level] + [(level, heading[2])], []
        else:
            lines.append(line)
    out.append((trail, "\n".join(lines)))
    return [([h for _, h in t], body) for t, body in out if body.strip()]


def paragraphs(sections, depth):
    """The text of sections, one folded paragraph a line, with each heading
    below depth, the first time it appears, as a line of its own."""
    lines, shown = [], []
    for trail, body in sections:
        for n in range(depth, len(trail)):
            if shown[:n + 1] != trail[:n + 1]:
                lines.append(trail[n])
        shown = trail
        lines += [" ".join(p.split()) for p in re.split(r"\n\s*\n", body) if p.strip()]
    return lines


def passages(text, title):
    """Text cut into passages of up to about PASSAGE_CHARS, each starting with a
    line "<title> > <heading> > <subheading>". A file or a section that fits stays
    whole, so a short note keeps its context; a longer one is cut at its
    subheadings, and a section without any at its paragraphs."""
    out = []

    def cut(trail, group):
        depth = len(trail)
        head = " > ".join([title, *trail])
        lines = paragraphs(group, depth)
        deeper = [s for s in group if len(s[0]) > depth]
        if len("\n".join(lines)) <= PASSAGE_CHARS or not deeper:
            chunk = ""
            for piece in (p for line in lines for p in split_long(line)):
                if chunk and len(chunk) + 1 + len(piece) > PASSAGE_CHARS:
                    out.append(f"{head}\n{chunk}")
                    chunk = ""
                chunk = f"{chunk}\n{piece}" if chunk else piece
            if chunk:
                out.append(f"{head}\n{chunk}")
            return
        # Text before the first subheading, then each subheading with what it holds.
        runs = []
        for section in group:
            key = section[0][:depth + 1]
            if runs and runs[-1][0] == key:
                runs[-1][1].append(section)
            else:
                runs.append((key, [section]))
        for key, run in runs:
            cut(key, run)

    cut([], sections(text))
    return out


def file_pages(f, suffix):
    """[(page, text)] of a binary file, page 0 when it has no pages, and its count of pages without text."""
    if suffix == ".pdf":
        pages = [(n, (page.extract_text() or "").strip()) for n, page in enumerate(open_pdf(f).pages, 1)]
        return [(n, t) for n, t in pages if t], sum(1 for _, t in pages if not t)
    if suffix in DOCUMENT_SUFFIXES:
        text = document_text(f, suffix)[:MAX_FILE_CHARS]
    else:
        text = f.read(MAX_FILE_CHARS * 4).decode("utf-8", errors="replace")[:MAX_FILE_CHARS]
    return ([(0, text)] if text.strip() else []), 0


def vectors(client, texts):
    """Raw vectors for texts, in order. A text the server refuses, as one past
    the model's 2048 tokens (music notation takes several a character), is
    embedded by its first half, down to MIN_EMBED_CHARS."""
    resp = client.post("/v1/embeddings", json={"input": texts})
    if resp.status_code == 500 and len(texts) > 1:
        return [v for t in texts for v in vectors(client, [t])]
    if resp.status_code == 500 and len(texts[0]) > MIN_EMBED_CHARS:
        return vectors(client, [texts[0][:len(texts[0]) // 2]])
    resp.raise_for_status()
    return [d["embedding"] for d in sorted(resp.json()["data"], key=lambda d: d["index"])]


def embed(client, texts):
    """Unit vectors for texts, in order, as float32 rows."""
    rows = []
    for i in range(0, len(texts), BATCH):
        rows += vectors(client, texts[i:i + BATCH])
    matrix = numpy.asarray(rows, dtype=numpy.float32)
    return matrix / numpy.linalg.norm(matrix, axis=1, keepdims=True)


def sha256(f):
    """The hash of a binary file, which is left at its start."""
    h = hashlib.sha256()
    for block in iter(lambda: f.read(1 << 20), b""):
        h.update(block)
    f.seek(0)
    return h.hexdigest()


def walk(tree):
    """({shown path: Item} of every file the index can use, {folders and labels
    Nextcloud did not list}). Hidden names, links out and what the filters
    leave out are not used."""
    found, unreachable = {}, set()
    usable = TEXT_SUFFIXES | DOCUMENT_SUFFIXES | IMAGE_FORMATS.keys() | {".pdf"}
    for label in tree.roots:
        try:
            top = tree.item(label, [])
            items = list(tree.walk(top)) if top and top.is_dir else []
        except NextcloudError as e:
            print(f"{label}: {e} What the index holds of it stays.", file=sys.stderr, flush=True)
            unreachable.add(label)
            continue
        found.update((i.shown, i) for i in items
                     if not i.is_dir and os.path.splitext(i.shown)[1].lower() in usable)
    for shown in tree.unlisted:
        print(f"{shown}: Nextcloud did not list it. What the index holds of it stays.", file=sys.stderr, flush=True)
    return found, unreachable | set(tree.unlisted)


def drop(db, file_id):
    db.execute("DELETE FROM passages WHERE file = ?", (file_id,))
    db.execute("DELETE FROM files WHERE id = ?", (file_id,))


def index_file(db, client, item, f, digest):
    """Store one file's passages and vectors; returns its status. Embedding errors propagate."""
    shown = item.shown
    suffix = os.path.splitext(shown)[1].lower()
    status, empty, error, rows = "indexed", 0, None, []
    if suffix in IMAGE_FORMATS:
        status = "needs_ocr"
    else:
        try:
            pages, empty = file_pages(f, suffix)
        except Exception as e:  # any reader error: the file is marked, the run goes on
            pages, status, error = [], "failed", f"{type(e).__name__}: {e}"[:300]
        rows = [(page, text) for page, whole in pages for text in passages(whole, os.path.basename(shown))]
        if status == "indexed" and not rows:
            status = "needs_ocr" if suffix == ".pdf" else "empty"
    vectors = embed(client, [DOC_PROMPT.format(title=os.path.basename(shown), text=t) for _, t in rows]) if rows else []
    cur = db.execute("INSERT INTO files (path, size, mtime_ns, ctime_ns, sha256, status, empty_pages, error) "
                     "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                     (shown, item.size, item.mtime_ns, item.ctime_ns, digest, status, empty, error))
    db.executemany("INSERT INTO passages (file, page, text, vector) VALUES (?, ?, ?, ?)",
                   [(cur.lastrowid, page, text, vec.tobytes()) for (page, text), vec in zip(rows, vectors)])
    return status


def read_status(base):
    """<base>.status.json, or {} when missing or unreadable."""
    try:
        with open(base + ".status.json", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def write_status(base, **fields):
    """Merge fields into <base>.status.json in one step."""
    path = base + ".status.json"
    status = read_status(base)
    status.update(fields)
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        json.dump(status, f)
    os.replace(path + ".tmp", path)


def publish(db, base):
    """Copy the working index to <base>.sqlite in one step, stamped with the
    time, and its files by status to the status file."""
    db.execute("INSERT OR REPLACE INTO meta VALUES ('updated', ?)", (time.strftime("%Y-%m-%d %H:%M"),))
    db.commit()
    dest = sqlite3.connect(base + ".sqlite.tmp")
    db.backup(dest)
    dest.close()
    os.replace(base + ".sqlite.tmp", base + ".sqlite")
    write_status(base, files=dict(db.execute("SELECT status, count(*) FROM files GROUP BY status")))


def quiet_now(quiet):
    """Whether the hour is within quiet hours "<from>-<to>", to excluded."""
    if not quiet:
        return False
    a, b = (int(h) % 24 for h in quiet.split("-"))
    hour = time.localtime().tm_hour
    return a <= hour < b if a < b else hour >= a or hour < b


def index_user(db, folders, rules, nextcloud, client, model, base, quiet=""):
    """Bring one user's index in line with their folders. Returns ({outcome: count},
    {labels Nextcloud did not list}). Stops at the quiet hours with what it has
    done published; counts then holds "paused"."""
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

    cache = {path: json.loads(listing) for path, listing in db.execute("SELECT path, listing FROM nextcloud_folders")}
    tree = Folders(folders, rules, nextcloud, cache)
    found, unreachable = walk(tree)
    # What Nextcloud did not list is kept as it was, neither dropped nor taken as moved.
    known = {r[0]: r[1:] for r in db.execute("SELECT path, id, size, mtime_ns, ctime_ns, sha256, error FROM files")
             if not any(r[0].startswith(u + "/") for u in unreachable)}
    gone = {row[4]: row[0] for path, row in known.items() if path not in found}
    reused, counts, last_publish = set(), {}, time.monotonic()
    for shown, item in sorted(found.items()):
        # The rest waits for the next run, which skips what this one did.
        if quiet_now(quiet):
            counts["paused"] = 1
            publish(db, base)
            return counts, unreachable
        row = known.get(shown)
        try:
            # A file failed by the embedding server, not by its content, is tried again.
            if row and row[1:4] == (item.size, item.mtime_ns, item.ctime_ns) \
                    and not (row[5] or "").startswith("HTTPStatusError"):
                outcome = "unchanged"
            else:
                with tree.open(item, MAX_FETCH_BYTES) as f:
                    digest = sha256(f)
                    moved = gone.pop(digest, None) if row is None else None
                    reused.add(moved)
                    if moved is not None or (row and row[4] == digest):
                        db.execute("UPDATE files SET path = ?, size = ?, mtime_ns = ?, ctime_ns = ? WHERE id = ?",
                                   (shown, item.size, item.mtime_ns, item.ctime_ns,
                                    moved if moved is not None else row[0]))
                        outcome = "moved" if moved is not None else "unchanged"
                    else:
                        if row:
                            drop(db, row[0])
                        outcome = index_file(db, client, item, f, digest)
        except httpx2.TransportError:
            raise
        except NextcloudError as e:
            # Not the file's fault: kept as it was, and tried again next run.
            db.rollback()
            print(f"{shown}: {e}", file=sys.stderr, flush=True)
            unreachable.add(shown.split("/", 1)[0])
            outcome = "skipped"
        except (OSError, ValueError, httpx2.HTTPStatusError) as e:
            # Kept as failed with its dates, so it is retried once it changes.
            db.rollback()
            if row:
                drop(db, row[0])
            db.execute("INSERT INTO files (path, size, mtime_ns, ctime_ns, status, error) "
                       "VALUES (?, ?, ?, ?, 'failed', ?)", (shown, item.size, item.mtime_ns,
                                                            item.ctime_ns, f"{type(e).__name__}: {e}"[:300]))
            outcome = "failed"
        db.commit()
        counts[outcome] = counts.get(outcome, 0) + 1
        if time.monotonic() - last_publish > PUBLISH_EVERY_S:
            publish(db, base)
            last_publish = time.monotonic()
    for path, row in known.items():
        if path not in found and row[0] not in reused:
            drop(db, row[0])
            counts["dropped"] = counts.get("dropped", 0) + 1
    # The listings of this run only, so a folder gone or filtered out is not kept.
    if not unreachable:
        db.execute("DELETE FROM nextcloud_folders")
    db.executemany("INSERT OR REPLACE INTO nextcloud_folders VALUES (?, ?)",
                   [(path, json.dumps(listing)) for path, listing in tree.listed.items()])
    publish(db, base)
    return counts, unreachable


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
    users, rules, _ = load_folders(os.environ["DOCS_FOLDERS_FILE"])
    nextcloud = Nextcloud.from_env(os.environ)
    needs_nextcloud(users, nextcloud)
    index_dir = os.environ["DOCS_INDEX_DIR"]
    # A password refused before is not tried again: each try counts against loopback.
    if nextcloud and any(read_status(os.path.join(index_dir, u)).get("nextcloud_refused") == nextcloud.mark
                         for u in users):
        nextcloud.refused = True
    quiet = os.environ.get("DOCS_QUIET_HOURS", "")
    try:
        os.remove(os.path.join(index_dir, "run-now"))
        quiet = ""
    except FileNotFoundError:
        pass
    if quiet_now(quiet):
        # A timer catching up after a stop: the next run outside them will do.
        print(f"quiet hours {quiet}: no run now.", flush=True)
        return
    broken = []
    with httpx2.Client(base_url=os.environ["DOCS_EMBED_URL"], timeout=300) as client:
        wait_ready(client)
        for user, folders in sorted(users.items()):
            base = os.path.join(index_dir, user)
            db = connect(base + ".work.sqlite")
            write_status(base, started=time.strftime("%Y-%m-%d %H:%M"), ended=None, result="running", run={}, error=None)
            try:
                counts, unreachable = index_user(db, folders, rules[user], nextcloud, client,
                                                 f"{os.environ['DOCS_EMBED_MODEL']}/passages-{PASSAGES_VERSION}", base, quiet)
                paused = counts.pop("paused", 0)
                print(f"{user}: " + (", ".join(f"{n} {k}" for k, n in sorted(counts.items())) or "no files")
                      + (f", paused for the quiet hours {quiet}" if paused else ""), flush=True)
                write_status(base, ended=time.strftime("%Y-%m-%d %H:%M"),
                             nextcloud_refused=nextcloud.mark if nextcloud and nextcloud.refused else None,
                             result="failed" if unreachable else "paused" if paused else "done",
                             run=counts, error=f"Nextcloud did not list {', '.join(sorted(unreachable))}." if unreachable else None)
                if unreachable:
                    broken.append(user)
            except httpx2.TransportError:
                write_status(base, ended=time.strftime("%Y-%m-%d %H:%M"), result="failed",
                             error="the embedding server stopped answering.")
                raise
            except Exception as e:  # one user's broken index must not stop the others
                print(f"{user}: index failed: {type(e).__name__}: {e}", file=sys.stderr, flush=True)
                write_status(base, ended=time.strftime("%Y-%m-%d %H:%M"), result="failed", error=f"{type(e).__name__}: {e}"[:300])
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
