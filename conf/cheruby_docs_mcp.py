"""MCP server: list, find, read and search one user's homelab folders, read-only.

Run by cheruby-docs-mcp.service. Settings come from its root-600 env file:

    DOCS_BIND               address to listen on, default 127.0.0.1
    DOCS_PORT               default 8770
    DOCS_TOKEN_<user>       a request carrying "Authorization: Bearer <it>" acts as <user>
    DOCS_FOLDERS_<user>     that user's folders, "<label>=<path>" joined by ":",
                            e.g. Documents=/srv/docs:Notes=/srv/notes
    DOCS_INDEX_DIR          where docs_index.py keeps <user>.sqlite, read here only
    DOCS_EMBED_URL          the embedding server, e.g. http://127.0.0.1:8771

The AI gateway in front holds one entry per user, each with that user's token,
so a key granted docs_<user> reaches that user's folders and nobody else's. Paths in
and out of the tools start with a folder's label, never with a real path.

Nothing here writes. A path is refused if it climbs out of its folder,
follows a link out of it, or names a hidden file (Syncthing's .stfolder,
.stversions). The unit adds the outer wall: the process sees only these
folders and the index, read-only.

The tools are served at /mcp over Streamable HTTP, stateless.
"""

import asyncio
import hmac
import logging
import os
import sqlite3
import sys
import unicodedata
from datetime import datetime

import httpx2
import numpy
import uvicorn
from mcp.server.mcpserver import Context, Image, MCPServer
from mcp.server.mcpserver.exceptions import ToolError

from docs_files import IMAGE_FORMATS, OFFICE_MEMBERS, TEXT_SUFFIXES, hidden, inside, load_folders, office_text, open_pdf
from docs_index import QUERY_PROMPT, connect, folders_key

BIND = os.environ.get("DOCS_BIND", "127.0.0.1")
PORT = int(os.environ.get("DOCS_PORT", "8770"))
INDEX_DIR = os.environ.get("DOCS_INDEX_DIR", "")
EMBED_URL = os.environ.get("DOCS_EMBED_URL", "")

MAX_LIST = 300
MAX_FOUND = 50
MAX_CHARS = 20_000
MAX_PDF_PAGES = 20
MAX_IMAGE_BYTES = 4_000_000
MAX_RESULTS = 10
RRF_K = 60


def load_users(environ):
    """{token: (user, {label: real path})} from DOCS_TOKEN_<user> and DOCS_FOLDERS_<user>."""
    folders = load_folders(environ)
    users = {}
    for key, token in environ.items():
        if not key.startswith("DOCS_TOKEN_"):
            continue
        name = key.removeprefix("DOCS_TOKEN_")
        if len(token) < 32:
            sys.exit(f"{key} is shorter than 32 characters.")
        if token in users:
            sys.exit(f"{key} repeats another user's token.")
        users[token] = (name, folders.get(name, {}))
    if not users:
        sys.exit("No DOCS_TOKEN_<user> is set.")
    return users


USERS = load_users(os.environ)

# One line per embedding request would fill the journal.
logging.getLogger("httpx2").setLevel(logging.WARNING)

mcp = MCPServer(
    "docs",
    instructions="The user's own files, read-only. To find a document by what it says, "
    "use search_documents; by its name, find_files; to browse, list_folder. Then read_file. "
    "Paths start with a folder name such as Documents/. Cite the paths you used.",
)


def folders_of(ctx):
    return ctx.request_context.request.scope["state"]["docs_folders"]


def user_of(ctx):
    return ctx.request_context.request.scope["state"]["docs_user"]


def resolve(ctx, path):
    """The real path behind a tool path, or a ToolError. "" means the folder list."""
    parts = [p for p in path.strip().strip("/").split("/") if p not in ("", ".")]
    if not parts:
        return None
    if any(p == ".." or hidden(p) or "\0" in p for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    folders = folders_of(ctx)
    root = folders.get(parts[0])
    if root is None:
        raise ToolError(f"{path}: no folder named {parts[0]!r}. Folders: {', '.join(folders)}.")
    real = os.path.realpath(os.path.join(root, *parts[1:]))
    if not inside(real, root):
        raise ToolError(f"{path}: leads out of {parts[0]} or into a hidden folder.")
    if not os.path.exists(real):
        raise ToolError(f"{path}: not found.")
    return real


def describe(real, shown):
    """One listing line: kind, size, date, path."""
    st = os.stat(real)
    date = datetime.fromtimestamp(st.st_mtime).strftime("%Y-%m-%d")
    if os.path.isdir(real):
        return f"dir             {date}  {shown}/"
    return f"file {human(st.st_size):>9}  {date}  {shown}"


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
    real = resolve(ctx, path)
    if real is None:
        return "\n".join(f"dir  {label}/" for label in folders_of(ctx))
    if not os.path.isdir(real):
        raise ToolError(f"{path}: is a file; read it with read_file.")
    shown = path.strip().strip("/")
    entries = sorted((e for e in os.scandir(real) if not hidden(e.name)),
                     key=lambda e: (not e.is_dir(), e.name.lower()))
    lines = [describe(e.path, f"{shown}/{e.name}") for e in entries[:MAX_LIST]]
    if len(entries) > MAX_LIST:
        lines.append(f"[{len(entries) - MAX_LIST} more not shown; use find_files]")
    return "\n".join(lines) or f"{shown}/ is empty."


def fold(text):
    """Lower case without accents, so "factures energie" finds "Factures_Énergie"."""
    return "".join(c for c in unicodedata.normalize("NFKD", text.lower()) if not unicodedata.combining(c))


@mcp.tool()
async def find_files(ctx: Context, query: str, path: str = "") -> str:
    """Find files and folders whose path contains every word of the query.

    Matching ignores case and accents. Searches names only, not contents.
    path: limit the search to one folder, such as "Documents/Santé".
    """
    words = fold(query).replace("_", " ").replace("-", " ").split()
    if not words:
        raise ToolError("query is empty.")
    real = resolve(ctx, path)
    starts = list(folders_of(ctx).items()) if real is None else [(path.strip().strip("/"), real)]
    found = []
    for shown_root, root in starts:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = sorted(d for d in dirnames if not hidden(d))
            rel = os.path.relpath(dirpath, root)
            for name in dirnames + sorted(f for f in filenames if not hidden(f)):
                shown = "/".join(p for p in (shown_root, "" if rel == "." else rel, name) if p)
                if all(w in fold(shown).replace("_", " ").replace("-", " ") for w in words):
                    found.append(describe(os.path.join(dirpath, name), shown))
                    if len(found) > MAX_FOUND:
                        return "\n".join(found[:MAX_FOUND]) + "\n[more matches; narrow the query]"
    return "\n".join(found) or f"Nothing matches {query!r}."


def cut(text, offset, max_chars):
    """A slice of text with a note saying how to read on."""
    total = len(text)
    part = text[offset:offset + max_chars]
    if offset + max_chars < total:
        part += f"\n\n[characters {offset}-{offset + len(part)} of {total}; continue with offset={offset + max_chars}]"
    return part


def pdf_text(real, page, pages):
    reader = open_pdf(real)
    total = len(reader.pages)
    if not 1 <= page <= total:
        raise ToolError(f"page {page} is out of range; the PDF has {total} pages.")
    last = min(total, page + pages - 1)
    chunks = []
    for n in range(page, last + 1):
        text = (reader.pages[n - 1].extract_text() or "").strip()
        chunks.append(f"--- page {n} ---\n{text or '[no text on this page: a scan or an image]'}")
    head = f"{total} pages; pages {page}-{last} below."
    if last < total:
        head += f" Continue with page={last + 1}."
    return head + "\n\n" + "\n\n".join(chunks)


@mcp.tool()
async def read_file(ctx: Context, path: str, page: int = 1, pages: int = 5,
                    offset: int = 0, max_chars: int = 8000) -> str | Image:
    """Read one file: text, Markdown, PDF, Word (.docx), OpenDocument (.odt) or an image.

    PDFs are read by page: page is the first one, pages how many (1 to 20).
    Other text is read by character: offset is where to start, max_chars how much
    (up to 20000). An image comes back as the picture itself. A scanned PDF has
    no text yet.
    """
    real = resolve(ctx, path)
    if real is None or os.path.isdir(real):
        raise ToolError(f"{path or '(empty)'}: is a folder; list it with list_folder.")
    suffix = os.path.splitext(real)[1].lower()
    max_chars = max(1, min(max_chars, MAX_CHARS))
    offset = max(0, offset)

    if suffix in IMAGE_FORMATS:
        if os.path.getsize(real) > MAX_IMAGE_BYTES:
            raise ToolError(f"{path}: the image is larger than {human(MAX_IMAGE_BYTES)}.")
        with open(real, "rb") as f:
            return Image(data=f.read(), format=IMAGE_FORMATS[suffix])
    try:
        if suffix == ".pdf":
            return await asyncio.to_thread(pdf_text, real, page, max(1, min(pages, MAX_PDF_PAGES)))
        if suffix in OFFICE_MEMBERS:
            return cut(await asyncio.to_thread(office_text, real), offset, max_chars)
    except ValueError as e:
        raise ToolError(f"{path}: {e}") from None
    if suffix in TEXT_SUFFIXES:
        with open(real, encoding="utf-8", errors="replace") as f:
            text = f.read()
        return cut(text, offset, max_chars)
    raise ToolError(f"{path}: {suffix or 'this file type'} cannot be read as text.")


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
    finds a letter that never uses those words. Returns the best passages, each
    with its file, page and date; read more with read_file.
    path: limit the search to one folder, such as "Documents/Logement".
    after, before: the file's date, as 2025-03-31.
    limit: how many passages, 1 to 10.
    Scans and images are not searchable yet; find them with find_files.
    """
    query = query.strip()
    if not query:
        raise ToolError("query is empty.")
    db_path = os.path.join(INDEX_DIR, f"{user_of(ctx)}.sqlite")
    if not os.path.exists(db_path):
        raise ToolError("the search index is not built yet; use find_files meanwhile.")
    parts = [p for p in path.strip().strip("/").split("/") if p not in ("", ".")]
    if any(p == ".." or hidden(p) for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    clauses, args = ["f.status = 'indexed'"], []
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
    try:
        meta = dict(db.execute("SELECT key, value FROM meta"))
        # Built for other folders: it may hold text this user no longer reaches.
        if meta.get("folders") != folders_key(folders_of(ctx)):
            raise ToolError("the search index is being rebuilt after a change of folders; use find_files meanwhile.")
        ids = ranked(db, query, vector, where, args)[:max(1, min(limit, MAX_RESULTS))]
        rows = {r[0]: r[1:] for r in db.execute(
            f"SELECT p.id, f.path, p.page, f.mtime_ns, p.text FROM passages p JOIN files f ON f.id = p.file "
            f"WHERE p.id IN ({','.join('?' * len(ids))})", ids)}
    finally:
        db.close()
    head = f"[index updated {meta.get('updated', '?')}]\n"
    if vector is None:
        head += "[search by meaning is unavailable; keyword matches only]\n"
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
