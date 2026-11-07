"""MCP server: list, find and read one user's homelab folders, read-only.

Run by cheruby-docs-mcp.service. Settings come from its root-600 env file:

    DOCS_BIND               address to listen on, default 127.0.0.1
    DOCS_PORT               default 8770
    DOCS_TOKEN_<user>       a request carrying "Authorization: Bearer <it>" acts as <user>
    DOCS_FOLDERS_<user>     that user's folders, "<label>=<path>" joined by ":",
                            e.g. Documents=/srv/docs:Notes=/srv/notes

The AI gateway in front holds one entry per user, each with that user's token,
so a key granted docs_<user> reaches that user's folders and nobody else's. Paths in
and out of the tools start with a folder's label, never with a real path.

Nothing here writes. A path is refused if it climbs out of its folder,
follows a link out of it, or names a hidden file (Syncthing's .stfolder,
.stversions). The unit adds the outer wall: the process sees only these
folders, read-only.

The tools are served at /mcp over Streamable HTTP, stateless.
"""

import asyncio
import hmac
import logging
import os
import sys
import unicodedata
import zipfile
from datetime import datetime
from xml.etree import ElementTree

import uvicorn
from mcp.server.mcpserver import Context, Image, MCPServer
from mcp.server.mcpserver.exceptions import ToolError
from pypdf import PdfReader

# pypdf warns on every odd font; the journal needs its errors only.
logging.getLogger("pypdf").setLevel(logging.ERROR)

BIND = os.environ.get("DOCS_BIND", "127.0.0.1")
PORT = int(os.environ.get("DOCS_PORT", "8770"))

MAX_LIST = 300
MAX_FOUND = 50
MAX_CHARS = 20_000
MAX_PDF_PAGES = 20
MAX_IMAGE_BYTES = 4_000_000
MAX_XML_BYTES = 20_000_000
TEXT_SUFFIXES = {".md", ".txt", ".csv", ".json", ".vcf", ".ics", ".xml", ".html", ".htm", ".yml", ".yaml", ".log"}
IMAGE_FORMATS = {".jpg": "jpeg", ".jpeg": "jpeg", ".png": "png", ".webp": "webp", ".gif": "gif"}


def load_users(environ):
    """{token: {label: real path}} from DOCS_TOKEN_<user> and DOCS_FOLDERS_<user>."""
    users = {}
    for key, token in environ.items():
        if not key.startswith("DOCS_TOKEN_"):
            continue
        name = key.removeprefix("DOCS_TOKEN_")
        if len(token) < 32:
            sys.exit(f"{key} is shorter than 32 characters.")
        if token in users:
            sys.exit(f"{key} repeats another user's token.")
        folders = {}
        for entry in filter(None, environ.get(f"DOCS_FOLDERS_{name}", "").split(":")):
            label, sep, path = entry.partition("=")
            if not sep or not label or "/" in label or label.startswith(".") or label in folders:
                sys.exit(f"DOCS_FOLDERS_{name}: bad or repeated entry {entry!r}.")
            folders[label] = os.path.realpath(path)
        users[token] = folders
    if not users:
        sys.exit("No DOCS_TOKEN_<user> is set.")
    return users


USERS = load_users(os.environ)

mcp = MCPServer(
    "docs",
    instructions="The user's own files, read-only. Start with list_folder or find_files, "
    "then read_file. Paths start with a folder name such as Documents/. "
    "Cite the paths you used.",
)


def folders_of(ctx):
    return ctx.request_context.request.scope["state"]["docs_folders"]


def resolve(ctx, path):
    """The real path behind a tool path, or a ToolError. "" means the folder list."""
    parts = [p for p in path.strip().strip("/").split("/") if p not in ("", ".")]
    if not parts:
        return None
    if any(p == ".." or p.startswith(".") or "\0" in p for p in parts):
        raise ToolError(f"{path}: hidden names and .. are not allowed.")
    folders = folders_of(ctx)
    root = folders.get(parts[0])
    if root is None:
        raise ToolError(f"{path}: no folder named {parts[0]!r}. Folders: {', '.join(folders)}.")
    real = os.path.realpath(os.path.join(root, *parts[1:]))
    if real != root and not real.startswith(root + os.sep):
        raise ToolError(f"{path}: leads out of {parts[0]}.")
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
    entries = sorted((e for e in os.scandir(real) if not e.name.startswith(".")),
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
            dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
            rel = os.path.relpath(dirpath, root)
            for name in dirnames + sorted(f for f in filenames if not f.startswith(".")):
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


def office_text(real, member):
    """Paragraphs of a docx (word/document.xml) or odt (content.xml)."""
    with zipfile.ZipFile(real) as z:
        if z.getinfo(member).file_size > MAX_XML_BYTES:
            raise ToolError("the document's text is too large to read.")
        root = ElementTree.fromstring(z.read(member))
    paragraphs = (("".join(el.itertext())).strip() for el in root.iter()
                  if el.tag.rsplit("}", 1)[-1] in ("p", "h"))
    return "\n".join(p for p in paragraphs if p)


def pdf_text(real, page, pages):
    reader = PdfReader(real)
    if reader.is_encrypted and not reader.decrypt(""):
        raise ToolError("the PDF is password-protected.")
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
    if suffix == ".pdf":
        return await asyncio.to_thread(pdf_text, real, page, max(1, min(pages, MAX_PDF_PAGES)))
    if suffix in (".docx", ".odt"):
        member = "word/document.xml" if suffix == ".docx" else "content.xml"
        text = await asyncio.to_thread(office_text, real, member)
        return cut(text, offset, max_chars)
    if suffix in TEXT_SUFFIXES:
        with open(real, encoding="utf-8", errors="replace") as f:
            text = f.read()
        return cut(text, offset, max_chars)
    raise ToolError(f"{path}: {suffix or 'this file type'} cannot be read as text.")


class RequireToken:
    """ASGI wrapper: the bearer token picks the user, or the request gets a 401.

    The user's folders go into the request's state, where the tools find them.
    """

    def __init__(self, app):
        self.app = app
        self.tokens = [(f"Bearer {token}".encode(), folders) for token, folders in USERS.items()]

    async def __call__(self, scope, receive, send):
        if scope["type"] == "http":
            given = dict(scope["headers"]).get(b"authorization", b"")
            folders = None
            for expected, candidate in self.tokens:
                if hmac.compare_digest(given, expected):
                    folders = candidate
            if folders is None:
                await send({"type": "http.response.start", "status": 401,
                            "headers": [(b"content-type", b"text/plain")]})
                await send({"type": "http.response.body", "body": b"missing or wrong token\n"})
                return
            scope.setdefault("state", {})["docs_folders"] = folders
        await self.app(scope, receive, send)


if __name__ == "__main__":
    app = mcp.streamable_http_app(stateless_http=True, json_response=True, host=BIND)
    uvicorn.run(RequireToken(app), host=BIND, port=PORT, log_level="info", proxy_headers=False)
