"""MCP server: list, find, read and search one user's homelab folders, and write in those marked writable.

Run by cheruby-docs-mcp.service. Settings come from its root-600 env file:

    DOCS_BIND               address to listen on, default 127.0.0.1
    DOCS_PORT               default 8770
    DOCS_TOKEN_<user>       a request carrying "Authorization: Bearer <it>" acts as <user>
    DOCS_FOLDERS_FILE       each user's folders and their filters (docs_files.load_folders)
    DOCS_GATEWAY_TOKEN      optional: a token for the AI gateway, acting as
    DOCS_GATEWAY_USER       this user, on
    DOCS_GATEWAY_FOLDERS    these of the user's folders, labels joined by ":"
    DOCS_INDEX_DIR          where docs_index.py keeps <user>.sqlite, read here only
    DOCS_EMBED_URL          the embedding server, e.g. http://127.0.0.1:8771
    DOCS_RERANK_URL         optional: the reranking server, e.g. http://127.0.0.1:8774
    DOCS_NEXTCLOUD_URL      when a folder is in Nextcloud: the user's WebDAV root,
    DOCS_NEXTCLOUD_HOST     the name Nextcloud answers to,
    DOCS_NEXTCLOUD_USER     the account and
    DOCS_NEXTCLOUD_PASSWORD its app password (docs_files.Nextcloud)

A token is one user's, with all or some of that user's folders; the gateway
token searches that user's index. Paths in and out of the tools start with a
folder's label, never with a real path.

Only write_file and create_folder write, and only in the folders the
folders file marks writable: they create a folder, or create or replace a
text file, never delete anything, never through a link. Nextcloud is asked to list and send files, and for those
folders to store one. A path is refused if it climbs out of its folder,
follows a link out of it, or names a hidden file (Syncthing's .stfolder,
.stversions); one the folder's filters leave out is not found. The unit adds
the outer wall: the process sees only these folders on disk and the index,
read-only but for the writable folders.

The tools are served at /mcp over Streamable HTTP, stateless.
"""

import asyncio
import hmac
import json
import logging
import os
import sys
import unicodedata
from datetime import datetime

import httpx2
import numpy
import uvicorn
from mcp.server.mcpserver import Context, Image, MCPServer
from mcp.server.mcpserver.exceptions import ToolError

from docs_files import (IMAGE_FORMATS, TEXT_SUFFIXES, Folders, Nextcloud, hidden, kept, load_folders,
                        needs_nextcloud)
from docs_text import DOCUMENT_SUFFIXES, document_text, pdf_pages
from docs_index import QUERY_PROMPT, connect, read_status

BIND = os.environ.get("DOCS_BIND", "127.0.0.1")
PORT = int(os.environ.get("DOCS_PORT", "8770"))
INDEX_DIR = os.environ.get("DOCS_INDEX_DIR", "")
EMBED_URL = os.environ.get("DOCS_EMBED_URL", "")
RERANK_URL = os.environ.get("DOCS_RERANK_URL", "")

MAX_LIST = 300
MAX_FOUND = 50
MAX_CHARS = 20_000
MAX_PDF_PAGES = 20
MAX_IMAGE_BYTES = 4_000_000
MAX_FETCH_BYTES = 64_000_000
MAX_RESULTS = 10
MAX_WRITE_BYTES = 2_000_000
RRF_K = 60
# Measured on the Pi (F-M-12): the 30 best of the fusion, each by its start,
# put the expected file in the top 5 for every test query in about 4 s. A
# passage starts with its file name and headings (docs_index.passages).
RERANK_CANDIDATES = 30
RERANK_CHARS = 360
RERANK_TIMEOUT_S = 20


def load_users(environ, folders):
    """{token: (user, {label: real path})} from the DOCS_TOKEN_* and DOCS_GATEWAY_* settings."""
    tokens = [(key, token, key.removeprefix("DOCS_TOKEN_"), folders.get(key.removeprefix("DOCS_TOKEN_"), {}))
              for key, token in environ.items() if key.startswith("DOCS_TOKEN_")]
    if environ.get("DOCS_GATEWAY_TOKEN"):
        name = environ.get("DOCS_GATEWAY_USER", "")
        own = folders.get(name, {})
        labels = [label for label in environ.get("DOCS_GATEWAY_FOLDERS", "").split(":") if label]
        if not set(labels) <= set(own):
            sys.exit(f"DOCS_GATEWAY_FOLDERS names folders {name!r} does not have.")
        tokens.append(("DOCS_GATEWAY_TOKEN", environ["DOCS_GATEWAY_TOKEN"], name, {label: own[label] for label in labels}))
    users = {}
    for key, token, name, granted in tokens:
        if len(token) < 32:
            sys.exit(f"{key} is shorter than 32 characters.")
        if token in users:
            sys.exit(f"{key} repeats another token.")
        users[token] = (name, granted)
    if not users:
        sys.exit("No DOCS_TOKEN_<user> is set.")
    return users


FOLDERS, RULES, WRITABLE = load_folders(os.environ["DOCS_FOLDERS_FILE"])
USERS = load_users(os.environ, FOLDERS)
NEXTCLOUD = Nextcloud.from_env(os.environ)
needs_nextcloud(FOLDERS, NEXTCLOUD)
# The indexer notes a refused password; it is not tried again until a new one restarts the server.
if NEXTCLOUD and any(read_status(os.path.join(INDEX_DIR, user)).get("nextcloud_refused") == NEXTCLOUD.mark
                     for user in FOLDERS):
    NEXTCLOUD.refused = True

# One line per embedding request would fill the journal.
logging.getLogger("httpx2").setLevel(logging.WARNING)

mcp = MCPServer(
    "docs",
    instructions="The user's own files. To find a document by what it says, "
    "use search_documents; by its name, find_files; to browse, list_folder. Then read_file. "
    "Paths start with a folder name such as Documents/. Cite the paths you used."
    + (" Folders listed as writable also take write_file and create_folder; the others are read-only." if any(WRITABLE.values()) else ""),
)


def folders_of(ctx):
    return ctx.request_context.request.scope["state"]["docs_folders"]


def user_of(ctx):
    return ctx.request_context.request.scope["state"]["docs_user"]


def folders(ctx):
    return Folders(folders_of(ctx), RULES[user_of(ctx)], NEXTCLOUD)


def split(path):
    return [p for p in path.strip().strip("/").split("/") if p not in ("", ".")]


def resolve(ctx, path):
    """The Item behind a tool path, or a ToolError. "" means the folder list, None."""
    parts = split(path)
    if not parts:
        return None
    if any(p == ".." or hidden(p) or "\0" in p for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    if parts[0] not in folders_of(ctx):
        raise ToolError(f"{path}: no folder named {parts[0]!r}. Folders: {', '.join(folders_of(ctx))}.")
    # A filter also hides that a file exists: left out reads as missing.
    try:
        item = folders(ctx).item(parts[0], parts[1:])
    except (ValueError, OSError) as e:
        raise ToolError(f"{path}: {e}") from None
    if item is None:
        raise ToolError(f"{path}: not found.")
    return item


def describe(item):
    """One listing line: kind, size, date, path."""
    date = datetime.fromtimestamp(item.mtime_ns / 1e9).strftime("%Y-%m-%d")
    if item.is_dir:
        return f"dir             {date}  {item.shown}/"
    return f"file {human(item.size):>9}  {date}  {item.shown}"


def human(size):
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024


@mcp.tool()
async def list_folder(ctx: Context, path: str = "") -> str:
    """List one folder: its subfolders, then its files with size and date.

    path: a folder such as "Documents/Factures"; empty lists the top folders.
    """
    # Off the event loop: a Nextcloud that hangs must not stall the other requests.
    folder = await asyncio.to_thread(resolve, ctx, path)
    if folder is None:
        return "\n".join(f"dir  {label}/" + (" (writable)" if label in WRITABLE[user_of(ctx)] else "")
                         for label in folders_of(ctx))
    if not folder.is_dir:
        raise ToolError(f"{path}: is a file; read it with read_file.")
    try:
        items = await asyncio.to_thread(folders(ctx).children, folder)
    except OSError as e:
        raise ToolError(f"{path}: {e}") from None
    items.sort(key=lambda i: (not i.is_dir, i.shown.lower()))
    lines = [describe(i) for i in items[:MAX_LIST]]
    if len(items) > MAX_LIST:
        lines.append(f"[{len(items) - MAX_LIST} more not shown; use find_files]")
    return "\n".join(lines) or f"{folder.shown}/ is empty."


def fold(text):
    """Lower case without accents, so "factures energie" finds "Factures_Énergie"."""
    return "".join(c for c in unicodedata.normalize("NFKD", text.lower()) if not unicodedata.combining(c))


def find(ctx, words, start):
    """Description lines of what matches every word, MAX_FOUND + 1 at most,
    then a line per folder Nextcloud did not list."""
    found, notes = [], []
    tree = folders(ctx)
    starts = [start] if start else []
    for label in folders_of(ctx) if start is None else ():
        try:
            starts.append(tree.item(label, []))
        except OSError as e:
            notes.append(f"[{label}: {e}]")
    for top in starts:
        try:
            items = list(tree.walk(top)) if top else []
        except OSError as e:
            notes.append(f"[{top.shown}: {e}]")
            continue
        for item in items:
            if all(w in fold(item.shown).replace("_", " ").replace("-", " ") for w in words):
                found.append(describe(item))
                if len(found) > MAX_FOUND:
                    return found, notes
    return found, notes + [f"[{shown}: Nextcloud did not list it]" for shown in tree.unlisted]


@mcp.tool()
async def find_files(ctx: Context, query: str, path: str = "") -> str:
    """Find files and folders whose path contains every word of the query.

    Matching ignores case and accents. Searches names only, not contents.
    path: limit the search to one folder, such as "Documents/Santé".
    """
    words = fold(query).replace("_", " ").replace("-", " ").split()
    if not words:
        raise ToolError("query is empty.")
    start = await asyncio.to_thread(resolve, ctx, path)
    found, notes = await asyncio.to_thread(find, ctx, words, start)
    if len(found) > MAX_FOUND:
        return "\n".join(found[:MAX_FOUND]) + "\n[more matches; narrow the query]"
    return "\n".join(found + notes) or f"Nothing matches {query!r}."


def cut(text, offset, max_chars):
    """A slice of text with a note saying how to read on."""
    total = len(text)
    part = text[offset:offset + max_chars]
    if offset + max_chars < total:
        part += f"\n\n[characters {offset}-{offset + len(part)} of {total}; continue with offset={offset + max_chars}]"
    return part


def pdf_text(source, page, pages):
    total, texts = pdf_pages(source, page, pages)
    if not 1 <= page <= total:
        raise ToolError(f"page {page} is out of range; the PDF has {total} pages.")
    last = page + len(texts) - 1
    chunks = [f"--- page {n} ---\n{text.strip() or '[no text on this page: a scan or an image]'}"
              for n, text in enumerate(texts, page)]
    head = f"{total} pages; pages {page}-{last} below."
    if last < total:
        head += f" Continue with page={last + 1}."
    return head + "\n\n" + "\n\n".join(chunks)


def read_item(tree, item, suffix, page, pages, offset, max_chars):
    """What read_file returns for a file Item."""
    with tree.open(item, MAX_IMAGE_BYTES if suffix in IMAGE_FORMATS else MAX_FETCH_BYTES) as f:
        if suffix in IMAGE_FORMATS:
            return Image(data=f.read(), format=IMAGE_FORMATS[suffix])
        if suffix == ".pdf":
            return pdf_text(f, page, pages)
        if suffix in DOCUMENT_SUFFIXES:
            return cut(document_text(f, suffix), offset, max_chars)
        return cut(f.read().decode("utf-8", errors="replace"), offset, max_chars)


@mcp.tool()
async def read_file(ctx: Context, path: str, page: int = 1, pages: int = 5,
                    offset: int = 0, max_chars: int = 8000) -> str | Image:
    """Read one file: text, Markdown, PDF, Word, Excel, PowerPoint, OpenDocument,
    EPUB, email (.eml) or an image.

    PDFs are read by page: page is the first one, pages how many (1 to 20).
    Other text is read by character: offset is where to start, max_chars how much
    (up to 20000). An image comes back as the picture itself. A scanned PDF has
    no text yet.
    """
    item = await asyncio.to_thread(resolve, ctx, path)
    if item is None or item.is_dir:
        raise ToolError(f"{path or '(empty)'}: is a folder; list it with list_folder.")
    suffix = os.path.splitext(item.shown)[1].lower()
    if suffix not in IMAGE_FORMATS.keys() | DOCUMENT_SUFFIXES | TEXT_SUFFIXES | {".pdf"}:
        raise ToolError(f"{path}: {suffix or 'this file type'} cannot be read as text.")
    if suffix in IMAGE_FORMATS and item.size > MAX_IMAGE_BYTES:
        raise ToolError(f"{path}: the image is larger than {human(MAX_IMAGE_BYTES)}.")
    try:
        return await asyncio.to_thread(read_item, folders(ctx), item, suffix, page,
                                       max(1, min(pages, MAX_PDF_PAGES)), max(0, offset), max(1, min(max_chars, MAX_CHARS)))
    except FileNotFoundError:
        raise ToolError(f"{path}: not found.") from None
    except (ValueError, OSError) as e:
        raise ToolError(f"{path}: {e}") from None


def ranked(db, query, vector, where, args):
    """Passage ids, best first, by meaning and by keywords, merged by reciprocal rank."""
    scores = {}
    if vector is not None:
        rows = db.execute(f"SELECT p.id, p.vector FROM passages p JOIN files f ON f.id = p.file {where}", args).fetchall()
        if rows:
            matrix = numpy.frombuffer(b"".join(r[1] for r in rows), dtype=numpy.float32).reshape(len(rows), -1)
            best = numpy.argsort(-(matrix @ vector))[:50]
            for rank, i in enumerate(best):
                scores[rows[i][0]] = 1 / (RRF_K + rank)
    words = [w for w in query.split() if any(c.isalnum() for c in w)]
    if words:
        match = " OR ".join('"' + w.replace('"', '""') + '"' for w in words)
        rows = db.execute(f"SELECT p.id FROM passages_fts JOIN passages p ON p.id = passages_fts.rowid "
                          f"JOIN files f ON f.id = p.file {where} AND passages_fts MATCH ? "
                          f"ORDER BY bm25(passages_fts) LIMIT 50", [*args, match]).fetchall()
        for rank, (pid,) in enumerate(rows):
            scores[pid] = scores.get(pid, 0) + 1 / (RRF_K + rank)
    return sorted(scores, key=scores.get, reverse=True)


async def embed_query(query):
    """The query's vector, or None when the embedding server does not answer."""
    try:
        async with httpx2.AsyncClient(timeout=30) as client:
            resp = await client.post(f"{EMBED_URL}/v1/embeddings", json={"input": [QUERY_PROMPT + query]})
            resp.raise_for_status()
    except httpx2.HTTPError:
        return None
    vector = numpy.asarray(resp.json()["data"][0]["embedding"], dtype=numpy.float32)
    return vector / numpy.linalg.norm(vector)


async def rerank(query, candidates):
    """Indexes of candidates, (path, page, mtime_ns, text) rows, most relevant to
    the query first; None when the reranking server does not answer."""
    docs = [text[:RERANK_CHARS] for _, _, _, text in candidates]
    try:
        async with httpx2.AsyncClient(timeout=RERANK_TIMEOUT_S) as client:
            resp = await client.post(f"{RERANK_URL}/v1/rerank", json={"query": query, "documents": docs})
            resp.raise_for_status()
        scored = sorted(resp.json()["results"], key=lambda r: -r["relevance_score"])
        order = [r["index"] for r in scored if isinstance(r["index"], int) and 0 <= r["index"] < len(docs)]
    except (httpx2.HTTPError, ValueError, KeyError, TypeError):
        return None
    return order + [i for i in range(len(docs)) if i not in order]


def day(text, name):
    try:
        return datetime.strptime(text, "%Y-%m-%d").timestamp() * 1e9
    except ValueError:
        raise ToolError(f"{name}: {text!r} is not a date like 2025-03-31.") from None


@mcp.tool()
async def search_documents(ctx: Context, query: str, path: str = "", after: str = "",
                           before: str = "", limit: int = 8) -> str:
    """Find passages of documents by what they say, in the user's words.

    Searches by meaning and by keywords, so "the letter about the rent increase"
    finds a letter that never uses those words. Returns the best passage of
    each of the best files, with its page and date; read more with read_file.
    path: limit the search to one folder, such as "Documents/Logement".
    after, before: the file's date, as 2025-03-31.
    limit: how many files, 1 to 10.
    Scans and images are not searchable yet; find them with find_files.
    """
    query = query.strip()
    if not query:
        raise ToolError("query is empty.")
    db_path = os.path.join(INDEX_DIR, f"{user_of(ctx)}.sqlite")
    if not os.path.exists(db_path):
        raise ToolError("the search index is not built yet; use find_files meanwhile.")
    parts = split(path)
    if any(p == ".." or hidden(p) for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    folders = folders_of(ctx)
    if parts and parts[0] not in folders:
        raise ToolError(f"{path}: no folder named {parts[0]!r}. Folders: {', '.join(folders)}.")
    # The index holds all the user's folders; a token may open only some of them.
    clauses = ["f.status = 'indexed'", "kept(f.path)",
               f"substr(f.path, 1, instr(f.path, '/') - 1) IN ({','.join('?' * len(folders))})"]
    args = list(folders)
    if parts:
        prefix = "/".join(parts)
        clauses.append("(f.path = ? OR substr(f.path, 1, ?) = ?)")
        args += [prefix, len(prefix) + 1, prefix + "/"]
    if after:
        clauses.append("f.mtime_ns >= ?")
        args.append(day(after, "after"))
    if before:
        clauses.append("f.mtime_ns < ?")
        args.append(day(before, "before") + 86_400e9)
    where = "WHERE " + " AND ".join(clauses)

    vector = await embed_query(query)
    db = connect(db_path, read_only=True)
    # Passages of files a filter now leaves out, until the next index run drops them.
    db.create_function("kept", 1, lambda shown, rules=RULES[user_of(ctx)]: kept(rules, shown), deterministic=True)
    try:
        meta = dict(db.execute("SELECT key, value FROM meta"))
        # A label now on another path: its text in the index is not what read_file
        # reaches. A folder the index does not know yet just has no passages.
        indexed = json.loads(meta.get("folders", "{}"))
        if any(indexed.get(label, real) != real for label, real in folders.items()):
            raise ToolError("the search index is being rebuilt after a change of folders; use find_files meanwhile.")
        ids = ranked(db, query, vector, where, args)[:50]
        rows = {r[0]: r[1:] for r in db.execute(
            f"SELECT p.id, f.path, p.page, f.mtime_ns, p.text FROM passages p JOIN files f ON f.id = p.file "
            f"WHERE p.id IN ({','.join('?' * len(ids))})", ids)}
    finally:
        db.close()
    # Overlapping passages of one page are one candidate, its best one.
    seen, pages = set(), []
    for pid in ids:
        if rows[pid][:2] not in seen:
            seen.add(rows[pid][:2])
            pages.append(pid)
    order = None
    if RERANK_URL and len(pages) > 1:
        order = await rerank(query, [rows[pid] for pid in pages[:RERANK_CANDIDATES]])
        if order is not None:
            pages = [pages[i] for i in order] + pages[RERANK_CANDIDATES:]
    # The best passage stands for its file, so one long document cannot fill the list.
    files, best = set(), []
    for pid in pages:
        if rows[pid][0] not in files:
            files.add(rows[pid][0])
            best.append(pid)
    ids = best[:max(1, min(limit, MAX_RESULTS))]
    head = f"[index updated {meta.get('updated', '?')}]\n"
    if vector is None:
        head += "[search by meaning is unavailable; keyword matches only]\n"
    if RERANK_URL and len(pages) > 1 and order is None:
        head += "[ranking by relevance is unavailable; results in a rougher order]\n"
    head += "\n"
    if not ids:
        return head + f"Nothing matches {query!r}."
    blocks = []
    for pid in ids:
        shown, page, mtime_ns, text = rows[pid]
        where_in = f"{shown}, page {page}" if page else shown
        date = datetime.fromtimestamp(mtime_ns / 1e9).strftime("%Y-%m-%d")
        blocks.append(f"--- {where_in} ({date}) ---\n{text}")
    return head + "\n\n".join(blocks)


def writable_parts(ctx, path):
    """The parts of a path in a folder this token may write in, or a ToolError."""
    parts = split(path)
    if any(p == ".." or hidden(p) or "\0" in p for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    writable = [label for label in folders_of(ctx) if label in WRITABLE[user_of(ctx)]]
    if len(parts) < 2:
        raise ToolError(f"{path or '(empty)'}: name something inside a writable folder: {', '.join(writable) or 'none'}.")
    if parts[0] not in folders_of(ctx):
        raise ToolError(f"{path}: no folder named {parts[0]!r}. Writable folders: {', '.join(writable) or 'none'}.")
    if parts[0] not in writable:
        raise ToolError(f"{parts[0]} is read-only. Writable folders: {', '.join(writable) or 'none'}.")
    return parts


def write(ctx, path, content):
    """What write_file returns, or, with content None, create_folder; or a ToolError."""
    parts = writable_parts(ctx, path)
    data = None
    if content is not None:
        suffix = os.path.splitext(parts[-1])[1].lower()
        if suffix not in TEXT_SUFFIXES:
            raise ToolError(f"{path}: only text files can be written: {', '.join(sorted(TEXT_SUFFIXES))}.")
        data = content.encode("utf-8")
        if len(data) > MAX_WRITE_BYTES:
            raise ToolError(f"{path}: the content is larger than {human(MAX_WRITE_BYTES)}.")
    try:
        outcome = folders(ctx).write(parts[0], parts[1:], data)
    except ValueError as e:
        raise ToolError(f"{path}: {e}") from None
    except OSError as e:
        # strerror leaves the real path out of the message.
        raise ToolError(f"{path}: {e.strerror + '.' if e.strerror else e}") from None
    shown = "/".join(parts)
    if outcome == "exists":
        return f"{shown}/ already exists."
    if data is None:
        return f"Created {shown}/."
    return f"{outcome.capitalize()} {shown}, {human(len(data))}."


async def write_file(ctx: Context, path: str, content: str) -> str:
    """Write a text file in a folder listed as writable: create it, with any
    missing folders, or replace it whole. Nothing is ever deleted.

    path: such as "Notes/Recettes/pain.md". Text files only (.md, .txt, .csv...).
    content: the whole new text of the file, UTF-8.
    """
    return await asyncio.to_thread(write, ctx, path, content)


async def create_folder(ctx: Context, path: str) -> str:
    """Create a folder, with any missing folders above it, in a folder listed
    as writable. A folder that exists already is left as it is.

    path: such as "Notes/Recettes/Pains".
    """
    return await asyncio.to_thread(write, ctx, path, None)


# Offered only where a folder takes them.
if any(WRITABLE.values()):
    mcp.tool()(write_file)
    mcp.tool()(create_folder)


class RequireToken:
    """ASGI wrapper: the bearer token picks the user, or the request gets a 401.

    The user's name and folders go into the request's state, where the tools find them.
    """

    def __init__(self, app):
        self.app = app
        self.tokens = [(f"Bearer {token}".encode(), user) for token, user in USERS.items()]

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http":
            given = dict(scope["headers"]).get(b"authorization", b"")
            user = None
            for expected, candidate in self.tokens:
                if hmac.compare_digest(given, expected):
                    user = candidate
            if user is None:
                await send({"type": "http.response.start", "status": 401,
                            "headers": [(b"content-type", b"text/plain")]})
                await send({"type": "http.response.body", "body": b"missing or wrong token\n"})
                return
            state = scope.setdefault("state", {})
            state["docs_user"], state["docs_folders"] = user
        await self.app(scope, receive, send)


if __name__ == "__main__":
    app = mcp.streamable_http_app(stateless_http=True, json_response=True, host=BIND)
    uvicorn.run(RequireToken(app), host=BIND, port=PORT, log_level="info", proxy_headers=False)
