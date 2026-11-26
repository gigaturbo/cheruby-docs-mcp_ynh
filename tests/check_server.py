"""Offline check of the tool server and the indexer. Run from the project root,
after cargo build --release in cheruby-docs/:

    .venv/bin/python tests/check_server.py

DOCS_BIN names another build of the binary, DOCS_PDFIUM another PDFium library
than the copy unpacked in cheruby-docs/target/pdfium/.

Builds two users' folders in a temporary directory, one of alice's served by
a stand-in Nextcloud, indexes them without filters against a stand-in
embedding server, starts the tool server, with a stand-in reranker, with filters on a loopback port,
then calls each tool and re-runs the indexer with the filters, then after a
move, an edit and a deletion, then with Nextcloud changed, down and refusing,
then embeds through a stand-in Whoopy, then writes in the writable folders,
on disk and in Nextcloud, then asks for index runs as an agent would, the
check starting each run as the path unit would, then transcribes recordings
with a stand-in whisper-server and Whoopy's speech route.
The stand-in embedder hashes words into vectors: it checks the wiring, not
the quality of the search. Needs httpx only; what the binary does inside a run
(the pause at the quiet hours, the halving of a text too long, one refusal
from Nextcloud, how passages are cut) is checked by cargo test.
Prints one line per check and exits 1 if any fails.
"""

import base64
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import urllib.parse
import zipfile
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx

ALICE, BOB, GATEWAY = "b" * 40, "j" * 40, "g" * 40
PORT = 18770
EMBED_PORT = 18771
RERANK_PORT = 18774
RERANK = {"favour": None, "down": False, "docs": []}
DAV_PORT = 18773
DAV_PASSWORD = "n" * 72
DAV_LOG = []
DAV = {"root": "", "down": False, "fail": None, "locked": set(), "bob": ""}
URL = f"http://127.0.0.1:{PORT}/mcp"
BIN = os.environ.get("DOCS_BIN", "cheruby-docs/target/release/cheruby-docs")
PDFIUM = os.environ.get("DOCS_PDFIUM", "cheruby-docs/target/pdfium/lib/libpdfium.so")
EMBEDDED = []
WHOOPY_PORT = 18776
WHOOPY_KEY = "w" * 40
WHOOPY = {"calls": 0, "left": -1, "skew": False, "gone": 503}
SPEECH_PORT = 18778
SPEECH = {"languages": [], "parts": 0, "fail": False}
WHOOPY_SPEECH = {"calls": 0, "busy": 0, "refuse": False}
PAGE_PORT = 18779
PANEL_TOKEN = "p" * 48
PAGE_ENV = {"DOCS_PAGE_PORT": str(PAGE_PORT), "DOCS_PANEL_TOKEN": PANEL_TOKEN, "DOCS_PAGE_USER": "alice"}
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")


def write_documents(docs):
    """One small file of each format document_text reads, besides docx, each with its own words."""
    ns = 'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" ' \
         'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" ' \
         'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
    rels = '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>'
    shape = '<p:sp><p:nvSpPr><p:cNvPr id="1" name="t"/><p:cNvSpPr/><p:nvPr>{}</p:nvPr></p:nvSpPr>' \
            '<p:txBody><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>'
    with zipfile.ZipFile(f"{docs}/cours.pptx", "w") as z:
        # Listed second, shown first: the order is the presentation's, not the file names'.
        z.writestr("ppt/presentation.xml", f'<p:presentation {ns}><p:sldIdLst><p:sldId id="256" r:id="rId2"/>'
                   '<p:sldId id="257" r:id="rId1"/></p:sldIdLst></p:presentation>')
        z.writestr("ppt/_rels/presentation.xml.rels", rels.format(
            '<Relationship Id="rId1" Target="slides/slide1.xml"/><Relationship Id="rId2" Target="/ppt/slides/slide2.xml"/>'))
        z.writestr("ppt/slides/slide2.xml", f'<p:sld {ns}><p:cSld><p:spTree>'
                   + shape.format('<p:ph type="ctrTitle"/>', "Tournesols") + shape.format("", "Semer en avril")
                   + "</p:spTree></p:cSld></p:sld>")
        z.writestr("ppt/slides/slide1.xml", f'<p:sld {ns}><p:cSld><p:spTree>'
                   + shape.format("", "Arroser le soir") + "</p:spTree></p:cSld></p:sld>")
    odf = 'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" ' \
          'xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" ' \
          'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" ' \
          'xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" ' \
          'xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0"'
    with zipfile.ZipFile(f"{docs}/mimosa.odp", "w") as z:
        z.writestr("content.xml", f'<office:document-content {odf}><office:body><office:presentation>'
                   '<draw:page><draw:frame presentation:class="title"><draw:text-box><text:p>Mimosa</text:p>'
                   '</draw:text-box></draw:frame><draw:frame><draw:text-box><text:p>Tailler après la floraison</text:p>'
                   '</draw:text-box></draw:frame><presentation:notes><draw:frame><draw:text-box>'
                   '<text:p>note secrete orateur</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>'
                   '</office:presentation></office:body></office:document-content>')
    with zipfile.ZipFile(f"{docs}/budget.xlsx", "w") as z:
        z.writestr("xl/workbook.xml", '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
                   'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
                   '<sheet name="Potager" sheetId="1" r:id="rId1"/></sheets></workbook>')
        z.writestr("xl/_rels/workbook.xml.rels", rels.format(
            '<Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId9" Target="sharedStrings.xml"/>'))
        z.writestr("xl/sharedStrings.xml", '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
                   '<si><t>Artichauts</t></si><si><r><t>gra</t></r><r><t>ines</t></r></si></sst>')
        z.writestr("xl/worksheets/sheet1.xml", '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
                   '<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>12.5</v></c>'
                   '<c r="C1" t="inlineStr"><is><t>euros</t></is></c></row><row r="2"/>'
                   '<row r="3"><c r="A3" t="s"><v>1</v></c></row></sheetData></worksheet>')
    with zipfile.ZipFile(f"{docs}/rhubarbe.ods", "w") as z:
        z.writestr("content.xml", f'<office:document-content {odf}><office:body><office:spreadsheet>'
                   '<table:table table:name="Récolte"><table:table-row><table:table-cell><text:p>Rhubarbe</text:p>'
                   '</table:table-cell><table:table-cell table:number-columns-repeated="3"/><table:table-cell>'
                   '<text:p>3 kg</text:p></table:table-cell></table:table-row>'
                   '<table:table-row table:number-rows-repeated="1000000"><table:table-cell/></table:table-row>'
                   '</table:table></office:spreadsheet></office:body></office:document-content>')
    with zipfile.ZipFile(f"{docs}/lavande.epub", "w") as z:
        z.writestr("mimetype", "application/epub+zip")
        z.writestr("META-INF/container.xml", '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">'
                   '<rootfiles><rootfile full-path="OEBPS/livre.opf"/></rootfiles></container>')
        z.writestr("OEBPS/livre.opf", '<package xmlns="http://www.idpf.org/2007/opf"><manifest>'
                   '<item id="c1" href="Texte/chap%201.xhtml"/><item id="c2" href="Texte/chap2.xhtml"/></manifest>'
                   '<spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>')
        z.writestr("OEBPS/Texte/chap2.xhtml", '<html><head><title>titre cache</title><style>p {}</style></head>'
                   '<body><h1>La lavande</h1><p>Elle aime le soleil&nbsp;et   la sécheresse.</p></body></html>')
        z.writestr("OEBPS/Texte/chap 1.xhtml", '<html><body><h2>Récolte</h2><p>Couper en juillet.</p></body></html>')
    with open(f"{docs}/glycine.eml", "w") as f:
        f.write("From: Jardin <jardin@example.org>\nTo: alice@example.org\nSubject: =?utf-8?q?Glycine_=C3=A0_tailler?=\n"
                "Date: Mon, 16 Nov 2026 09:00:00 +0100\nMIME-Version: 1.0\n"
                'Content-Type: multipart/mixed; boundary="b"\n\n--b\nContent-Type: multipart/alternative; boundary="c"\n\n'
                "--c\nContent-Type: text/plain; charset=utf-8\n\nTailler la glycine en août.\n"
                "--c\nContent-Type: text/html; charset=utf-8\n\n<p>version html</p>\n--c--\n"
                '--b\nContent-Type: application/pdf\nContent-Disposition: attachment; filename="plan.pdf"\n'
                "Content-Transfer-Encoding: base64\n\nJVBERg==\n--b--\n")
    with open(f"{docs}/abime.xlsx", "wb") as f:
        f.write(b"not a zip")
    with open(f"{docs}/rapport.pdf", "wb") as f:
        f.write(tiny_pdf(*(f"Rapport annuel CONFIDENTIEL\nCotisation {w} recal-\nculee au bureau {n}0\nPage {n} sur 4"
                           for n, w in enumerate(["ortie", "sureau", "tilleul", "verveine"], 1))))
    with open(f"{docs}/recette.html", "w") as f:
        f.write("<html><head><title>titre</title><style>.x { color: red }</style></head><body><h1>Tarte</h1>"
                "<p>P&acirc;te <b>bris\u00e9e</b> maison</p><script>var cache = 1;</script></body></html>")
    with open(f"{docs}/carnet.xml", "w") as f:
        f.write('<carnet><plante nom="x"><![CDATA[Semis de capucines]]></plante><plante>Bouture de figuier</plante></carnet>')


def tiny_pdf(*pages):
    """A PDF with one page per argument, each line of it shown on its own line,
    with a correct cross-reference table."""
    n_pages = len(pages)
    kids = " ".join(f"{4 + 2 * i} 0 R" for i in range(n_pages))
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               f"<< /Type /Pages /Kids [{kids}] /Count {n_pages} >>".encode(),
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for i, page in enumerate(pages):
        shown = " 0 -14 Td ".join(f"({line}) Tj" for line in page.split("\n"))
        stream = f"BT /F1 12 Tf 72 720 Td {shown} ET".encode()
        objects.append(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents %d 0 R "
                       b"/Resources << /Font << /F1 3 0 R >> >> >>" % (5 + 2 * i))
        objects.append(b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream")
    out, offsets = b"%PDF-1.4\n", []
    for n, body in enumerate(objects, 1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % n + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % o for o in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return out


class FakeEmbedder(BaseHTTPRequestHandler):
    """/health, and /v1/embeddings giving each text the normalised sum of its words' hashes.

    A text containing "panne" gets a 500, to see the server fall back on keywords,
    and one holding "♪" and longer than 600 characters, as a text past the
    model's tokens would.
    """

    def do_GET(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        self.answer(json.loads(self.rfile.read(int(self.headers["Content-Length"])))["input"])

    def answer(self, texts):
        EMBEDDED.extend(texts)
        if any("panne" in t or ("♪" in t and len(t) > 600) for t in texts):
            self.send_response(500)
            self.end_headers()
            return
        data = []
        for i, text in enumerate(texts):
            vector = [0.0] * 64
            for word in text.lower().split():
                vector[hashlib.sha256(word.encode()).digest()[0] % 64] += 1.0
            data.append({"index": i, "embedding": vector})
        body = json.dumps({"data": data}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def form(body, name):
    """A field of a multipart body, as text."""
    m = re.search(rb'name="' + re.escape(name.encode()) + rb'"\r\n\r\n(.*?)\r\n', body, re.S)
    return m.group(1).decode() if m else None


def transcribed(handler, language, text):
    """A verbose_json answer for the WAV part in the request: one segment over all of it."""
    body = handler.rfile.read(int(handler.headers["Content-Length"]))
    riff = body.index(b"RIFF")
    seconds = int.from_bytes(body[riff + 40:riff + 44], "little") / 32000
    answer = json.dumps({"language": language,
                         "segments": [{"start": 0.0, "end": seconds, "text": text}]}).encode()
    handler.send_response(200)
    handler.send_header("Content-Type", "application/json")
    handler.end_headers()
    handler.wfile.write(answer)
    return body


class FakeWhisper(BaseHTTPRequestHandler):
    """whisper-server: /health, and /inference answering "fr" and one segment
    per part, the language it was sent kept in SPEECH."""

    def do_GET(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        SPEECH["parts"] += 1
        if SPEECH["fail"]:
            self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(500)
            self.end_headers()
            return
        body = transcribed(self, "fr", " Partie.")
        SPEECH["languages"].append(form(body, "language"))

    def log_message(self, *args):
        pass


class FakeWhoopy(FakeEmbedder):
    """Whoopy: 401 without its key and model, then the stand-in embedder's vectors,
    or those of each text reversed when skewed, and "gone" (503, or 500 as LiteLLM
    answers for a desktop out of reach) once "left" batches are spent.
    Its speech route answers 503 while "busy" lasts, as while the desktop
    wakes, 401 when refusing, and names the language in full."""

    def do_POST(self):
        if self.path == "/v1/audio/transcriptions":
            WHOOPY_SPEECH["calls"] += 1
            if WHOOPY_SPEECH["refuse"] or self.headers.get("Authorization") != f"Bearer {WHOOPY_KEY}":
                self.send_response(401)
                self.end_headers()
                return
            if WHOOPY_SPEECH["busy"] > 0:
                WHOOPY_SPEECH["busy"] -= 1
                self.send_response(503)
                self.end_headers()
                return
            transcribed(self, "french", " Bureau.")
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        WHOOPY["calls"] += 1
        refused = 401 if self.headers.get("Authorization") != f"Bearer {WHOOPY_KEY}" \
            or body.get("model") != "ollama/fake" else WHOOPY["gone"] if WHOOPY["left"] == 0 else None
        if refused:
            self.send_response(refused)
            self.end_headers()
            return
        WHOOPY["left"] -= 1
        self.answer([t[::-1] for t in body["input"]] if WHOOPY["skew"] else body["input"])


class FakeReranker(BaseHTTPRequestHandler):
    """/v1/rerank keeping the order it is given, but for the documents holding
    RERANK["favour"], which go first; a 500 while RERANK["down"]. The
    documents sent go to RERANK["docs"]."""

    def do_POST(self):
        docs = json.loads(self.rfile.read(int(self.headers["Content-Length"])))["documents"]
        RERANK["docs"] = docs
        if RERANK["down"]:
            self.send_response(500)
            self.end_headers()
            return
        favour = RERANK["favour"]
        results = [{"index": i, "relevance_score": (10 if favour and favour in d else 0) - i / 100}
                   for i, d in enumerate(docs)]
        body = json.dumps({"results": results[::-1]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def reranked(favour=None, down=False, **arguments):
    """A search with the stand-in reranker set as given, and the documents it got."""
    RERANK.update(favour=favour, down=down, docs=[])
    try:
        return (*tool("search_documents", **arguments), RERANK["docs"])
    finally:
        RERANK.update(favour=None, down=False)


class FakeNextcloud(BaseHTTPRequestHandler):
    """WebDAV on DAV["root"] as alice's account, at /dav/files/alice: PROPFIND
    at depth 0 or 1, GET, PUT, MKCOL and MOVE. A path in DAV["locked"] answers
    PUT and MOVE with 423, as a file Text holds open does. What lies under
    DAV["bob"] is owned by bob, as a share from him is. A folder's ETag changes with anything
    below it, as Nextcloud's does. Every request goes to DAV_LOG as (method,
    depth, path).
    """

    base = "/dav/files/alice"

    def etag(self, real):
        if not os.path.isdir(real):
            st = os.stat(real)
            return f"{st.st_size}-{st.st_mtime_ns}"
        inner = "".join(f"{n}:{self.etag(os.path.join(real, n))}" for n in sorted(os.listdir(real)))
        return hashlib.sha256(inner.encode()).hexdigest()[:16]

    def target(self, exists=True):
        """The real path asked for, or None after an answer was sent. With
        exists false, the path may be missing but its folder must not be."""
        path = urllib.parse.unquote(urllib.parse.urlsplit(self.path).path)
        DAV_LOG.append((self.command, self.headers.get("Depth", ""), path[len(self.base):].rstrip("/") or "/"))
        if DAV.get("throttle"):
            self.send_response(429)
            self.end_headers()
            return None
        if DAV["down"] or (DAV["fail"] and path.startswith(self.base + DAV["fail"])):
            self.send_response(503)
            self.end_headers()
            return None
        if self.headers.get("Authorization") != "Basic " + base64.b64encode(f"alice:{DAV_PASSWORD}".encode()).decode():
            self.send_response(401)
            self.end_headers()
            return None
        real = os.path.join(DAV["root"], path[len(self.base):].strip("/"))
        if not path.startswith(self.base) or not os.path.exists(real if exists else os.path.dirname(real)):
            self.send_response(404 if exists else 409)
            self.end_headers()
            return None
        return real

    def response(self, real):
        rel = os.path.relpath(real, DAV["root"])
        href = urllib.parse.quote(self.base + ("" if rel == "." else "/" + rel) + ("/" if os.path.isdir(real) else ""))
        kind = "<d:resourcetype><d:collection/></d:resourcetype>" if os.path.isdir(real) else \
            f"<d:resourcetype/><d:getcontentlength>{os.path.getsize(real)}</d:getcontentlength>"
        owner = "bob" if DAV["bob"] and (rel + "/").startswith(DAV["bob"] + "/") else "alice"
        return (f"<d:response><d:href>{href}</d:href><d:propstat><d:prop>{kind}"
                f"<oc:owner-id>{owner}</oc:owner-id>"
                f"<d:getlastmodified>{formatdate(os.path.getmtime(real), usegmt=True)}</d:getlastmodified>"
                f'<d:getetag>"{self.etag(real)}"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status>'
                f"</d:propstat></d:response>")

    def do_PROPFIND(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        real = self.target()
        if real is None:
            return
        found = [real]
        if self.headers.get("Depth") == "1" and os.path.isdir(real):
            found += [os.path.join(real, n) for n in sorted(os.listdir(real))]
        body = ('<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">'
                + "".join(self.response(r) for r in found) + "</d:multistatus>").encode()
        self.send_response(207)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        real = self.target()
        if real is None:
            return
        with open(real, "rb") as f:
            body = f.read()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self):
        data = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        real = self.target(exists=False)
        if real is None:
            return
        if self.locked():
            return
        existed = os.path.exists(real)
        with open(real, "wb") as f:
            f.write(data)
        self.send_response(204 if existed else 201)
        self.end_headers()

    def locked(self):
        if urllib.parse.unquote(urllib.parse.urlsplit(self.path).path)[len(self.base):] not in DAV["locked"]:
            return False
        self.send_response(423)
        self.end_headers()
        return True

    def do_MOVE(self):
        real = self.target()
        if real is None or self.locked():
            return
        dest = urllib.parse.unquote(urllib.parse.urlsplit(self.headers.get("Destination", "")).path)
        new = os.path.join(DAV["root"], dest[len(self.base):].strip("/"))
        if self.headers.get("Overwrite") != "F" or not dest.startswith(self.base):
            self.send_response(400)
        elif os.path.exists(new):
            self.send_response(412)
        elif not os.path.isdir(os.path.dirname(new)):
            self.send_response(409)
        else:
            os.rename(real, new)
            self.send_response(201)
        self.end_headers()

    def do_MKCOL(self):
        real = self.target(exists=False)
        if real is None:
            return
        if os.path.exists(real):
            self.send_response(405)
        else:
            os.mkdir(real)
            self.send_response(201)
        self.end_headers()

    def __getattr__(self, name):
        # Any other method, such as PUT or DELETE, is logged and refused.
        if name.startswith("do_"):
            return lambda: (self.target(), self.send_response(405), self.end_headers())
        raise AttributeError(name)

    def log_message(self, *args):
        pass


def write_folders(path, folders):
    """The folders file: {user: {label: (root, source[, writable])}}, a root in
    Nextcloud written as nextcloud:<path in the account>."""
    def entry(root, source, writable=False):
        where = {"nextcloud": root[len("nextcloud:"):]} if root.startswith("nextcloud:") else {"path": root}
        return {**where, "source": source, **({"writable": True} if writable else {})}
    with open(path, "w") as f:
        json.dump({user: {label: entry(*e) for label, e in entries.items()} for user, entries in folders.items()}, f)


def write_settings(path, filters=None, schedule="never", quiet="", cron=""):
    """The page's settings: {source: (only, skip)}. Never by default, so no
    scheduled request lands in the middle of a check."""
    with open(path, "w") as f:
        json.dump({"schedule": schedule, "cron": cron, "quiet": quiet,
                   "filters": {src: {"only": o, "skip": k} for src, (o, k) in (filters or {}).items()}}, f)


# Documents is a Syncthing folder, Notes and Papiers local ones, Cloud in Nextcloud.
FILTERS = {"syncthing": ([], ["brouillons", "/Documents/*.tmp.md"]),
           "local": (["*.md"], []),
           "nextcloud": ([], ["vieux"])}


def build(base):
    """alice: Documents and Notes, filtered, and Cloud in Nextcloud, Notes and
    Cloud writable. bob: Papiers.
    Returns the env for the server; the folders file without filters is for
    the first index run."""
    docs, notes, papiers = (os.path.join(base, d) for d in ("docs", "notes", "papiers"))
    os.makedirs(os.path.join(docs, "Factures"))
    os.makedirs(os.path.join(docs, "Brouillons"))
    os.makedirs(os.path.join(docs, ".stversions"))
    os.makedirs(notes)
    os.makedirs(papiers)
    files = {
        f"{docs}/Factures/Énergie_2024.md": "Facture EDF\n" + "x" * 30_000,
        f"{docs}/.stversions/old.md": "hidden",
        f"{docs}/.hidden.md": "hidden",
        f"{notes}/courses.md": "pain, lait",
        f"{notes}/long.md": " ".join(f"Phrase {n} sur le jardin et les tomates." for n in range(300)),
        f"{papiers}/bob.md": "bob only",
        f"{docs}/Brouillons/draft.md": "brouillon cache",
        f"{docs}/Factures/Copie.TMP.md": "copie cachee",
        f"{base}/secret.md": "outside every folder",
    }
    for path, text in files.items():
        with open(path, "w") as f:
            f.write(text)
    with open(f"{docs}/bail.pdf", "wb") as f:
        f.write(tiny_pdf("Loyer augmente de 3 pourcent"))
    with open(f"{docs}/scan.png", "wb") as f:
        f.write(PNG)
    with open(f"{notes}/photo.png", "wb") as f:
        f.write(PNG)
    with zipfile.ZipFile(f"{docs}/lettre.docx", "w") as z:
        z.writestr("word/document.xml",
                   '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
                   '<w:body><w:p><w:pPr><w:pStyle w:val="Titre1"/></w:pPr><w:r><w:t>Résiliation</w:t></w:r></w:p>'
                   "<w:p><w:r><w:t>Madame, </w:t></w:r><w:r><w:t>Monsieur</w:t></w:r></w:p>"
                   "<w:p><w:r><w:t>Objet : préavis</w:t></w:r></w:p></w:body></w:document>")
    write_documents(docs)
    os.symlink(f"{base}/secret.md", f"{docs}/escape.md")
    os.symlink(".stversions/old.md", f"{docs}/vieux.md")
    os.symlink("Brouillons", f"{docs}/Pub")
    os.symlink("Brouillons/draft.md", f"{docs}/alias.md")
    os.symlink(base, f"{notes}/dehors")
    os.utime(f"{notes}/courses.md", (946_000_000, 946_000_000))  # 1999-12-24
    os.makedirs(f"{base}/index/requests")
    DAV["root"] = f"{base}/nextcloud"
    for path, text in {"Partage/compte-rendu.md": "Réunion du conseil syndical, ravalement vote",
                       "Partage/.cache.md": "hidden",
                       "Partage/Vieux/old.md": "ancien compte rendu",
                       "Autre/prive.md": "outside the shared folder"}.items():
        os.makedirs(os.path.dirname(f"{base}/nextcloud/{path}"), exist_ok=True)
        with open(f"{base}/nextcloud/{path}", "w") as f:
            f.write(text)
    os.makedirs(f"{base}/nextcloud/Partage/Sous dossier")
    with open(f"{base}/nextcloud/Partage/Sous dossier/plan d'été.pdf", "wb") as f:
        f.write(tiny_pdf("Plan du jardin partage"))
    folders = {"alice": {"Documents": (docs, "syncthing"),
                         "Notes": (notes, "local", True),
                         "Cloud": ("nextcloud:/Partage", "nextcloud", True)},
               "ju": {"Papiers": (papiers, "local")}}
    write_folders(f"{base}/folders.json", folders)
    write_settings(f"{base}/settings.json", FILTERS)
    write_settings(f"{base}/open-settings.json")
    write_folders(f"{base}/documents.json", {"alice": {"Documents": folders["alice"]["Documents"]},
                                             "ju": folders["ju"]})
    write_folders(f"{base}/moved.json", {"alice": {**folders["alice"], "Notes": (papiers, "local")},
                                         "ju": folders["ju"]})
    return {
        "PATH": os.environ.get("PATH", ""),
        "DOCS_PDFIUM": os.path.abspath(PDFIUM),
        "DOCS_PORT": str(PORT),
        "DOCS_INDEX_DIR": f"{base}/index",
        "DOCS_EMBED_URL": f"http://127.0.0.1:{EMBED_PORT}",
        "DOCS_EMBED_MODEL": "fake-64",
        "DOCS_EMBED_NAME": "embeddinggemma",
        "DOCS_RERANK_URL": f"http://127.0.0.1:{RERANK_PORT}",
        "DOCS_FOLDERS_FILE": f"{base}/folders.json",
        "DOCS_SETTINGS_FILE": f"{base}/settings.json",
        "DOCS_TOKEN_alice": ALICE,
        "DOCS_TOKEN_ju": BOB,
        "DOCS_GATEWAY_TOKEN": GATEWAY,
        "DOCS_GATEWAY_USER": "alice",
        "DOCS_GATEWAY_FOLDERS": "Notes",
        "DOCS_NEXTCLOUD_URL": f"http://127.0.0.1:{DAV_PORT}/dav/files/alice",
        "DOCS_NEXTCLOUD_USER": "alice",
        "DOCS_NEXTCLOUD_PASSWORD": DAV_PASSWORD,
        "DOCS_SPEECH_URL": f"http://127.0.0.1:{SPEECH_PORT}",
        "DOCS_SPEECH_MODEL": "fake-small",
        "DOCS_SPEECH_ENGINE": "demand",
        "DOCS_SPEECH_REMOTE_URL": f"http://127.0.0.1:{WHOOPY_PORT}",
        "DOCS_SPEECH_REMOTE_MODEL": "speaches/fake",
        "DOCS_SPEECH_REMOTE_KEY": WHOOPY_KEY,
    }


def call(method, params=None, token=ALICE):
    headers = {"Accept": "application/json, text/event-stream"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    resp = httpx.post(URL, headers=headers, timeout=60,
                      json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}})
    if resp.status_code != 200:
        return resp.status_code, ""
    result = resp.json()["result"]
    if "tools" in result:
        return 200, " ".join(t["name"] for t in result["tools"])
    content = result["content"][0]
    status = "error" if result.get("isError") else "ok"
    return status, content.get("text") or content.get("mimeType", "")


def tool(name, token=ALICE, **arguments):
    return call("tools/call", {"name": name, "arguments": arguments}, token=token)


def ok_with(*needles):
    return lambda r: r[0] == "ok" and all(n in r[1] for n in needles)


def error_with(needle):
    return lambda r: r[0] == "error" and needle in r[1]


CHECKS = [
    ("no token gets 401", lambda: call("tools/list", token=""), lambda r: r[0] == 401),
    ("wrong token gets 401", lambda: call("tools/list", token="x" * 40), lambda r: r[0] == 401),
    ("nine tools listed", lambda: call("tools/list"),
     lambda r: sorted(r[1].split()) == sorted("list_folder find_files read_file search_documents write_file create_folder "
                                              "move_file run_index transcribe_file".split())),
    ("alice sees her three folders, which are writable", lambda: tool("list_folder"),
     lambda r: r == ("ok", "dir  Documents/\ndir  Notes/ (writable)\ndir  Cloud/ (writable)")),
    ("bob sees only his", lambda: tool("list_folder", token=BOB), lambda r: r == ("ok", "dir  Papiers/")),
    ("bob cannot reach alice's", lambda: tool("read_file", token=BOB, path="Notes/courses.md"), error_with("no folder named")),
    ("alice cannot reach bob's", lambda: tool("read_file", path="Papiers/bob.md"), error_with("no folder named")),
    ("folder listed, hidden left out", lambda: tool("list_folder", path="Documents"),
     lambda r: r[0] == "ok" and "Documents/Factures/" in r[1] and "bail.pdf" in r[1] and ".st" not in r[1] and ".hidden" not in r[1]),
    (".. refused", lambda: tool("read_file", path="Documents/../secret.md"), error_with("not allowed")),
    ("hidden refused", lambda: tool("read_file", path="Documents/.stversions/old.md"), error_with("not allowed")),
    ("link out refused", lambda: tool("read_file", path="Documents/escape.md"), error_with("leads out")),
    ("link into hidden refused", lambda: tool("read_file", path="Documents/vieux.md"), error_with("hidden folder")),
    ("filtered folder not listed", lambda: tool("list_folder", path="Documents"),
     lambda r: r[0] == "ok" and "Brouillons" not in r[1] and "Copie" not in r[1]),
    ("only keeps matching files", lambda: tool("list_folder", path="Notes"),
     lambda r: r[0] == "ok" and "courses.md" in r[1] and "photo.png" not in r[1]),
    ("filtered folder reads as missing", lambda: tool("list_folder", path="Documents/Brouillons"), error_with("not found")),
    ("file below it too", lambda: tool("read_file", path="Documents/Brouillons/draft.md"), error_with("not found")),
    ("a link to a filtered folder is not listed", lambda: tool("list_folder", path="Documents"),
     lambda r: r[0] == "ok" and "Pub" not in r[1] and "alias" not in r[1]),
    ("nor read through", lambda: tool("read_file", path="Documents/Pub/draft.md"), error_with("not found")),
    ("a link to a filtered file is not read", lambda: tool("read_file", path="Documents/alias.md"), error_with("not found")),
    ("nor found", lambda: tool("find_files", query="alias"), lambda r: r == ("ok", "Nothing matches 'alias'.")),
    ("nor the folder link", lambda: tool("find_files", query="pub"), lambda r: r[0] == "ok" and "Documents/Pub" not in r[1]),
    ("filter with a path ignores case", lambda: tool("read_file", path="Documents/Factures/Copie.TMP.md"),
     error_with("not found")),
    ("find leaves filtered out", lambda: tool("find_files", query="draft"), lambda r: r == ("ok", "Nothing matches 'draft'.")),
    ("find keeps to only", lambda: tool("find_files", query="photo"), lambda r: r == ("ok", "Nothing matches 'photo'.")),
    ("search hides what the index still holds", lambda: tool("search_documents", query="brouillon copie cache", limit=10),
     lambda r: r[0] == "ok" and "Brouillons" not in r[1] and "Copie" not in r[1]),
    ("find ignores accents and case", lambda: tool("find_files", query="factures energie"), ok_with("Documents/Factures/Énergie_2024.md")),
    ("find within a folder", lambda: tool("find_files", query="courses", path="Notes"), ok_with("Notes/courses.md")),
    ("find leaves hidden out", lambda: tool("find_files", query="old"), lambda r: r[0] == "ok" and ".stversions" not in r[1]),
    ("text cut with a way on", lambda: tool("read_file", path="Documents/Factures/Énergie_2024.md"),
     ok_with("Facture EDF", "continue with offset=8000")),
    ("text read on from offset", lambda: tool("read_file", path="Documents/Factures/Énergie_2024.md", offset=29_000),
     lambda r: r[0] == "ok" and "continue" not in r[1]),
    ("PDF read by page", lambda: tool("read_file", path="Documents/bail.pdf"), ok_with("1 pages", "Loyer augmente")),
    ("PDF page out of range", lambda: tool("read_file", path="Documents/bail.pdf", page=3), error_with("out of range")),
    ("docx paragraphs", lambda: tool("read_file", path="Documents/lettre.docx"), ok_with("Madame, Monsieur\nObjet : préavis")),
    ("docx headings as Markdown", lambda: tool("read_file", path="Documents/lettre.docx"), ok_with("# Résiliation\nMadame")),
    ("a short file is one passage, after its name, its headings kept", lambda: tool("search_documents", query="préavis"),
     ok_with("---\nlettre.docx\nRésiliation\nMadame, Monsieur")),
    ("pptx slides in the deck's order, titles as headings", lambda: tool("read_file", path="Documents/cours.pptx"),
     ok_with("# Slide 1: Tournesols\nSemer en avril\n# Slide 2\nArroser le soir")),
    ("odp slides without the speaker notes", lambda: tool("read_file", path="Documents/mimosa.odp"),
     lambda r: ok_with("# Slide 1: Mimosa\nTailler après la floraison")(r) and "secrete" not in r[1]),
    ("xlsx rows under the sheet's name", lambda: tool("read_file", path="Documents/budget.xlsx"),
     lambda r: r == ("ok", "# Potager\nArtichauts\t12.5\teuros\ngraines")),
    ("ods rows, repeated empty ones left out", lambda: tool("read_file", path="Documents/rhubarbe.ods"),
     lambda r: r == ("ok", "# Récolte\nRhubarbe\t3 kg")),
    ("epub chapters in reading order, without tags", lambda: tool("read_file", path="Documents/lavande.epub"),
     lambda r: r == ("ok", "# La lavande\nElle aime le soleil et la sécheresse.\n## Récolte\nCouper en juillet.")),
    ("eml headers, attachments and plain text", lambda: tool("read_file", path="Documents/glycine.eml"),
     lambda r: ok_with("# Glycine à tailler\nFrom: Jardin <jardin@example.org>", "Attachments: plan.pdf",
                       "Tailler la glycine en août.")(r) and "version html" not in r[1]),
    ("a PDF page read as it is, header and footer kept", lambda: tool("read_file", path="Documents/rapport.pdf", page=2, pages=1),
     ok_with("Rapport annuel CONFIDENTIEL\nCotisation sureau recalculee au bureau 20\nPage 2 sur 4")),
    ("a word cut at a line end is found whole", lambda: tool("search_documents", query="recalculee"),
     ok_with("--- Documents/rapport.pdf")),
    ("HTML is searched by its text", lambda: tool("search_documents", query="brisee"),
     lambda r: ok_with("--- Documents/recette.html (", "recette.html\nTarte\nPâte brisée maison")(r)
     and not any(w in r[1].split("--- Documents/recette.html")[1].split("\n--- ")[0] for w in ("<b>", "color", "cache"))),
    ("HTML is read as it is", lambda: tool("read_file", path="Documents/recette.html"), ok_with("<b>brisée</b>")),
    ("XML is searched by its text", lambda: tool("search_documents", query="capucines"),
     lambda r: ok_with("carnet.xml\nSemis de capucines Bouture de figuier")(r) and "<plante" not in r[1]),
    ("a damaged document is named", lambda: tool("read_file", path="Documents/abime.xlsx"), error_with("damaged")),
    ("search finds a slide", lambda: tool("search_documents", query="tournesols"), ok_with("--- Documents/cours.pptx")),
    ("search finds a spreadsheet row", lambda: tool("search_documents", query="rhubarbe"), ok_with("--- Documents/rhubarbe.ods")),
    ("search finds a book", lambda: tool("search_documents", query="lavande juillet"), ok_with("--- Documents/lavande.epub")),
    ("search finds an email", lambda: tool("search_documents", query="glycine"), ok_with("--- Documents/glycine.eml")),
    ("image comes back as a picture", lambda: tool("read_file", path="Documents/scan.png"), lambda r: r == ("ok", "image/png")),
    ("folder is not a file", lambda: tool("read_file", path="Documents/Factures"), error_with("is a folder")),
    ("search finds a PDF page", lambda: tool("search_documents", query="loyer augmente"),
     ok_with("[index updated 20", "--- Documents/bail.pdf, page 1 (", "Loyer augmente de 3 pourcent")),
    ("search ignores accents", lambda: tool("search_documents", query="preavis"), ok_with("Documents/lettre.docx")),
    ("search within a folder", lambda: tool("search_documents", query="loyer tomates", path="Notes"),
     lambda r: r[0] == "ok" and "Notes/long.md" in r[1] and "Documents/" not in r[1]),
    ("search before a date", lambda: tool("search_documents", query="pain lait tomates", before="2000-01-01"),
     lambda r: r[0] == "ok" and "Notes/courses.md" in r[1] and "long.md" not in r[1]),
    ("search limit kept", lambda: tool("search_documents", query="loyer jardin pain", limit=2), lambda r: r[1].count("--- ") == 2),
    ("one passage per file", lambda: tool("search_documents", query="jardin tomates", limit=10),
     lambda r: r[0] == "ok" and r[1].count("--- Notes/long.md") == 1),
    ("the reranker orders the results", lambda: reranked("courses.md", query="loyer augmente pain"),
     lambda r: r[0] == "ok" and r[1].split("\n--- ")[1].startswith("Notes/courses.md")),
    ("the reranker gets each candidate's file name and start", lambda: reranked(query="loyer augmente"),
     lambda r: r[2] and any(d.startswith("bail.pdf\nLoyer augmente") for d in r[2])
     and all(len(d) <= 360 for d in r[2])),
    ("the reranker sees only the token's folders", lambda: reranked(token=GATEWAY, query="loyer augmente tomates"),
     lambda r: r[2] and not any(d.startswith(("bail.pdf", "lettre.docx")) or "Loyer augmente" in d for d in r[2])),
    ("search goes on without the reranker", lambda: reranked(down=True, query="loyer augmente"),
     ok_with("ranking by relevance is unavailable", "--- Documents/bail.pdf")),
    ("search leaves hidden out", lambda: tool("search_documents", query="hidden", limit=10),
     lambda r: r[0] == "ok" and ".st" not in r[1] and ".hidden" not in r[1]),
    ("search refuses ..", lambda: tool("search_documents", query="x", path="Notes/.."), error_with("not allowed")),
    ("search bad date refused", lambda: tool("search_documents", query="x", after="hier"), error_with("not a date")),
    ("bob's search sees only his", lambda: tool("search_documents", token=BOB, query="loyer bob"),
     lambda r: r[0] == "ok" and "Papiers/bob.md" in r[1] and "Documents" not in r[1]),
    ("gateway sees only the folders it opens", lambda: tool("list_folder", token=GATEWAY), lambda r: r == ("ok", "dir  Notes/ (writable)")),
    ("gateway cannot read the others", lambda: tool("read_file", token=GATEWAY, path="Documents/bail.pdf"),
     error_with("no folder named")),
    ("gateway search uses the user's index, its folders only", lambda: tool("search_documents", token=GATEWAY, query="loyer tomates"),
     lambda r: r[0] == "ok" and "Notes/long.md" in r[1] and "Documents/" not in r[1]),
    ("gateway search refuses the others", lambda: tool("search_documents", token=GATEWAY, query="loyer", path="Documents"),
     error_with("no folder named")),
    ("search falls back on keywords", lambda: tool("search_documents", query="panne loyer"),
     ok_with("keyword matches only", "Documents/bail.pdf")),
    ("a Nextcloud folder is listed", lambda: tool("list_folder", path="Cloud"),
     lambda r: r[0] == "ok" and r[1].startswith("dir ") and "Cloud/Sous dossier/" in r[1]
     and "Cloud/compte-rendu.md" in r[1] and ".cache" not in r[1] and "Vieux" not in r[1]),
    ("a Nextcloud file is read", lambda: tool("read_file", path="Cloud/compte-rendu.md"), ok_with("conseil syndical")),
    ("a Nextcloud PDF is read", lambda: tool("read_file", path="Cloud/Sous dossier/plan d'été.pdf"), ok_with("Plan du jardin")),
    ("a filtered Nextcloud folder reads as missing", lambda: tool("read_file", path="Cloud/Vieux/old.md"), error_with("not found")),
    ("a missing Nextcloud file is not found", lambda: tool("read_file", path="Cloud/absent.md"), error_with("not found")),
    ("Nextcloud .. refused", lambda: tool("read_file", path="Cloud/../Autre/prive.md"), error_with("not allowed")),
    ("Nextcloud hidden refused", lambda: tool("read_file", path="Cloud/.cache.md"), error_with("not allowed")),
    ("find walks Nextcloud", lambda: tool("find_files", query="plan ete"), ok_with("Cloud/Sous dossier/plan d'été.pdf")),
    ("find leaves Nextcloud's filtered out", lambda: tool("find_files", query="old"),
     lambda r: r[0] == "ok" and "Vieux" not in r[1]),
    ("search finds a Nextcloud passage", lambda: tool("search_documents", query="conseil syndical"),
     ok_with("Cloud/compte-rendu.md")),
    ("the gateway does not see Nextcloud", lambda: tool("read_file", token=GATEWAY, path="Cloud/compte-rendu.md"),
     error_with("no folder named")),
]


def page(path, token=PANEL_TOKEN, body=None, site="same-origin", user="alice"):
    """The page through nginx: its token, the portal's user, and a browser's mark on a POST."""
    headers = {"X-Panel-Token": token} if token else {}
    if user:
        headers["Authorization"] = "Basic " + base64.b64encode(f"{user}:-".encode()).decode()
    url = f"http://127.0.0.1:{PAGE_PORT}/{path}"
    if body is None:
        return httpx.get(url, headers=headers, timeout=30)
    if site:
        headers["Sec-Fetch-Site"] = site
    return httpx.post(url, headers=headers, json=body, timeout=60)


def page_checks(base):
    """The page: its guard, Status, Search, and the run lock it reads."""
    yield "page: no panel token gets 401", page("").status_code, page("", token="").status_code == 401
    yield "page: a wrong one gets 401", None, page("api/status", token="q" * 48).status_code == 401
    yield "page: another portal user gets 403", None, page("api/status", user="ju").status_code == 403
    yield "page: no portal user gets 403", None, page("api/status", user=None).status_code == 403
    r = page("")
    yield "page: served with the token", r.status_code, r.status_code == 200 and "Cheruby Docs" in r.text
    s = page("api/status").json()
    labels = [f["label"] for f in s["folders"]]
    yield "page: Status shows alice's folders only", labels, sorted(labels) == ["Cloud", "Documents", "Notes"]
    yield "page: and which are writable", s["folders"], \
        {f["label"]: f["writable"] for f in s["folders"]} == {"Documents": False, "Notes": True, "Cloud": True}
    yield "page: the embedding server answers", s, s["embed"] is True and not s["running"] and s["index"]
    import fcntl
    with open(f"{base}/index/run.lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        held = page("api/status").json()
        fcntl.flock(lock, fcntl.LOCK_UN)
    yield "page: a run holding the lock shows as running", held["index"], \
        held["running"] is True and held["index"][0].startswith("- **Running now**")
    r = page("api/search", body={"query": "conseil syndical"}, site=None)
    yield "page: a POST without the page's mark gets 403", r.status_code, r.status_code == 403
    r = page("api/search", body={"query": "conseil syndical"}, site="cross-site")
    yield "page: a cross-site POST gets 403", r.status_code, r.status_code == 403
    r = page("api/search", body={"query": "conseil syndical"})
    hits = r.json().get("hits", []) if r.status_code == 200 else []
    yield "page: Search finds the passage", r.text[:300], any(h["place"] == "Cloud/compte-rendu.md" for h in hits)
    r = page("api/search", body={"query": "x" * 501})
    yield "page: a long query is refused", r.status_code, r.status_code == 400
    r = page("api/search", body={"query": "loyer", "path": "Papiers"})
    yield "page: bob's folder is not alice's", r.text[:200], r.status_code == 400 and "no folder named" in r.text
    r = page("api/search", body={"query": "loyer", "extra": 1})
    yield "page: unknown fields refused", r.status_code, r.status_code >= 400


def wav_bytes(seconds, quiet=()):
    """A 16 kHz mono WAV of loud noise, silent for half a second at each of quiet."""
    import io
    import wave
    noise = os.urandom(32000)
    data = bytearray(noise * seconds)
    for q in quiet:
        data[int(q * 32000):int((q + 0.5) * 32000)] = bytes(16000)
    out = io.BytesIO()
    with wave.open(out, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(bytes(data))
    return out.getvalue()


def speech_checks(base):
    """transcribe_file and the Transcribe tab, against the stand-in whisper-server and Whoopy."""
    rec = f"{base}/docs/Dictées"
    os.makedirs(rec)
    with open(f"{rec}/réunion.wav", "wb") as f:
        f.write(wav_bytes(21 * 60, quiet=(570, 1155)))
    for name in ("court.wav", "bureau.wav", "allemand.wav", "panne.wav"):
        with open(f"{rec}/{name}", "wb") as f:
            f.write(wav_bytes(5))
    with open(f"{base}/notes/memo.wav", "wb") as f:
        f.write(wav_bytes(2))
    result = tool("transcribe_file", path="Documents/Dictées/réunion.wav")
    lines = result[1].splitlines()
    yield "speech: a recording is transcribed on the Pi", result, result[0] == "ok" and lines[0] == \
        "Documents/Dictées/réunion.wav: 21:00 of audio, language fr, transcribed on the Pi (fake-small)."
    yield "speech: cut at the pauses, each part's time shifted", lines[2:], \
        lines[2:] == ["[0:00] Partie.", "[9:30] Partie.", "[19:15] Partie."]
    yield "speech: the language found on the first part is sent after", SPEECH["languages"], \
        SPEECH["languages"] == ["auto", "fr", "fr"]
    start = time.time()
    again = tool("transcribe_file", path="Documents/Dictées/réunion.wav", timestamps=False)
    yield "speech: a second call comes from the cache, as paragraphs", (again, SPEECH["parts"]), \
        again[0] == "ok" and again[1].endswith("\n\nPartie. Partie. Partie.") and SPEECH["parts"] == 3 \
        and time.time() - start < 3
    yield "speech: the cache is in the index folder", None, len(os.listdir(f"{base}/index/transcripts")) == 1
    result = tool("transcribe_file", path="Documents/Dictées/allemand.wav", language="German")
    yield "speech: a language asked for is sent from the first part", (result, SPEECH["languages"]), \
        result[0] == "ok" and SPEECH["languages"][-1] == "de" and "language de" in result[1]
    yield "speech: an unknown language is refused", None, \
        tool("transcribe_file", path="Documents/Dictées/court.wav", language="klingon")[0] == "error"
    yield "speech: a filtered recording is not found", None, \
        error_with("not found")(tool("transcribe_file", path="Notes/memo.wav"))
    yield "speech: a document is not a recording", None, \
        error_with("not a recording")(tool("transcribe_file", path="Documents/bail.pdf"))
    yield "speech: .. is refused", None, \
        error_with("not allowed")(tool("transcribe_file", path="Documents/../notes/memo.wav"))
    yield "speech: an unknown engine is refused", None, \
        error_with("is not pi or desktop")(tool("transcribe_file", path="Documents/Dictées/court.wav", engine="gpu"))
    yield "speech: bob cannot reach alice's", None, \
        error_with("no folder named")(tool("transcribe_file", token=BOB, path="Documents/Dictées/court.wav"))
    SPEECH["fail"] = True
    result = tool("transcribe_file", path="Documents/Dictées/panne.wav")
    SPEECH["fail"] = False
    yield "speech: a failure is said, with where", result, \
        result[0] == "error" and "answered 500" in result[1] and "failed at 0:00" in result[1]
    parts = SPEECH["parts"]
    result = tool("transcribe_file", path="Documents/Dictées/panne.wav")
    yield "speech: and said again, not tried again, without retry", result, \
        result[0] == "error" and "retry true" in result[1] and SPEECH["parts"] == parts
    result = tool("transcribe_file", path="Documents/Dictées/panne.wav", retry=True)
    yield "speech: retry tries again", result, result[0] == "ok" and "[0:00] Partie." in result[1]
    WHOOPY_SPEECH.update(busy=2, calls=0)
    result = tool("transcribe_file", path="Documents/Dictées/bureau.wav", engine="desktop")
    yield "speech: the desktop is retried while it wakes", (result, WHOOPY_SPEECH), \
        result[0] == "ok" and WHOOPY_SPEECH["calls"] == 3 \
        and "language fr, transcribed on the desktop (speaches/fake)." in result[1] and "[0:00] Bureau." in result[1]
    WHOOPY_SPEECH.update(refuse=True, calls=0)
    parts = SPEECH["parts"]
    result = tool("transcribe_file", path="Documents/Dictées/court.wav", engine="desktop")
    WHOOPY_SPEECH.update(refuse=False)
    yield "speech: a desktop that refuses leaves the rest to the Pi", result, \
        result[0] == "ok" and WHOOPY_SPEECH["calls"] == 1 and SPEECH["parts"] == parts + 1 \
        and "The desktop failed (Whoopy answered 401)" in result[1] and "[0:00] Partie." in result[1]
    s = page("api/status").json()
    yield "page: Status shows the speech server and both engines", s, \
        s["speech"] is True and s["engines"] == ["pi", "desktop"]
    r = page("api/transcribe", body={"path": "Documents/Dictées/réunion.wav"})
    key = r.json().get("key", "")
    d = page(f"api/transcribe/{key}").json() if key else {}
    yield "page: a folder's recording, from the cache", d, \
        d.get("state") == "done" and d["text"].startswith("[0:00] Partie.") and d["shown"].endswith("réunion.wav")
    r = page("api/transcribe", body={"path": "Papiers/bob.md"})
    yield "page: bob's folder is not alice's", r.text[:200], r.status_code == 400

    def upload(name, data, site="same-origin"):
        headers = {"X-Panel-Token": PANEL_TOKEN, "Sec-Fetch-Site": site, "X-File-Name": urllib.parse.quote(name),
                   "Authorization": "Basic " + base64.b64encode(b"alice:-").decode()}
        return httpx.post(f"http://127.0.0.1:{PAGE_PORT}/api/transcribe/upload", headers=headers,
                          content=data, timeout=60)
    r = upload("note vocale.wav", wav_bytes(3))
    key = r.json().get("key", "") if r.status_code == 200 else ""
    d = {}
    for _ in range(50):
        d = page(f"api/transcribe/{key}").json() if key else {}
        if d.get("state") in ("done", "failed"):
            break
        time.sleep(0.2)
    yield "page: an upload is transcribed", (r.text[:200], d), \
        d.get("state") == "done" and d["shown"] == "upload: note vocale.wav" and d["text"] == "[0:00] Partie."
    yield "page: an upload that is not a recording is refused", None, upload("x.txt", b"hello").status_code == 400
    yield "page: an upload without the page's mark gets 403", None, \
        upload("y.wav", wav_bytes(1), site="cross-site").status_code == 403
    yield "page: a bad key is refused", None, page("api/transcribe/zz").status_code == 400


def settings_body(filters=FILTERS, **fields):
    return {"schedule": "never", "cron": "", "quiet": "",
            "filters": {src: {"only": o, "skip": k} for src, (o, k) in filters.items()}, **fields}


def settings_checks(base, env):
    """The Config tab: the settings saved, refused, and in use at once."""
    d = page("api/settings").json()
    yield "settings: read as saved", d, d["settings"]["schedule"] == "never" \
        and d["sources"] == ["syncthing", "local", "nextcloud"] and d["next"].startswith("none")
    r = page("api/settings", body=settings_body(), site=None)
    yield "settings: a POST without the page's mark gets 403", r.status_code, r.status_code == 403
    r = page("api/settings", body=settings_body(), user="ju")
    yield "settings: another portal user gets 403", r.status_code, r.status_code == 403
    r = page("api/settings", body=settings_body(schedule="custom", cron="0 3 1 * 1"))
    yield "settings: a bad cron is refused", r.text[:200], r.status_code == 400 and "Not a cron schedule" in r.text
    r = page("api/settings", body=settings_body(schedule="custom", cron="0 3 * * *", quiet="01-06"))
    yield "settings: a schedule the quiet hours empty is refused", r.text[:200], \
        r.status_code == 400 and "no time to run" in r.text
    r = page("api/settings", body=settings_body(filters={"ftp": ([], [])}))
    yield "settings: an unknown source is refused", r.text[:200], r.status_code == 400
    r = page("api/settings", body={**settings_body(), "remote_url": "http://example.org"})
    yield "settings: unknown fields refused", r.status_code, r.status_code >= 400
    with open(f"{base}/settings.json") as f:
        yield "settings: nothing refused is saved", None, json.load(f) == settings_body()
    before = tool("list_folder", path="Notes")
    hidden = {**FILTERS, "local": (["*.md"], ["liste.md"])}
    r = page("api/settings", body=settings_body(hidden))
    after = tool("list_folder", path="Notes")
    yield "settings: a filter saved hides a file at once", (before, after), \
        r.status_code == 200 and "liste.md" in before[1] and "liste.md" not in after[1]
    yield "settings: and reading it says not found", None, error_with("not found")(tool("read_file", path="Notes/liste.md"))
    with open(f"{base}/settings.json") as f:
        on_disk = json.load(f)
    yield "settings: saved to the file, mode 600", on_disk, on_disk["filters"]["local"]["skip"] == ["liste.md"] \
        and os.stat(f"{base}/settings.json").st_mode & 0o777 == 0o600
    hour = time.localtime().tm_hour
    page("api/settings", body=settings_body(quiet=f"{hour:02d}-{(hour + 1) % 24:02d}"))
    result = tool("run_index", files=["Notes/fraise.md"])
    yield "settings: quiet hours saved refuse run_index at once", result, error_with("quiet hours")(result)
    r = page("api/settings", body=settings_body(schedule="hourly", quiet=""))
    yield "settings: the next run follows the schedule", r.text[:200], \
        r.status_code == 200 and ":20" in r.json()["next"]
    page("api/settings", body=settings_body())
    yield "settings: back as they were", None, "liste.md" in tool("list_folder", path="Notes")[1]
    # A due minute passed since the last run: a server starting asks for a run, as Persistent= did.
    late = f"{base}/late"
    os.makedirs(f"{late}/requests")
    with open(f"{late}/alice.status.json", "w") as f:
        json.dump({"started": "2000-01-01 00:00", "result": "done"}, f)
    write_settings(f"{late}/settings.json", schedule="hourly")
    other = subprocess.Popen([BIN, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                             env={**env, "DOCS_INDEX_DIR": late, "DOCS_SETTINGS_FILE": f"{late}/settings.json",
                                  "DOCS_PORT": "18781"})
    try:
        for _ in range(50):
            if os.listdir(f"{late}/requests"):
                break
            time.sleep(0.2)
    finally:
        other.terminate()
        said = other.communicate()[1].decode()
    asked = sorted(os.listdir(f"{late}/requests"))
    yield "schedule: a missed run is asked for at start, a full run per user", (asked, said[-300:]), \
        asked == ["schedule-alice.json", "schedule-ju.json"] \
        and json.load(open(f"{late}/requests/schedule-alice.json")) == {"user": "alice"}
    write_settings(f"{base}/broken.json", schedule="weekly")
    refused = subprocess.run([BIN, "serve"], capture_output=True, text=True,
                             env={**env, "DOCS_SETTINGS_FILE": f"{base}/broken.json", "DOCS_PORT": "18782"})
    yield "a settings file the page would refuse stops the server", refused.stderr[-200:], \
        refused.returncode != 0 and "hourly, 6h, nightly, custom or never" in refused.stderr


def run_indexer(env):
    out = subprocess.run([BIN, "index"], env=env, capture_output=True, text=True, timeout=120)
    return out.stdout + out.stderr


def statuses(base):
    db = sqlite3.connect(f"{base}/index/alice.sqlite")
    rows = dict(db.execute("SELECT path, status FROM files"))
    db.close()
    return rows


def index_checks(base, env, first):
    """Checks on the indexer's first run, then on a re-run after changes."""
    docs, notes = f"{base}/docs", f"{base}/notes"
    files = statuses(base)
    yield "first run indexes both users", first, "alice:" in first and "ju: 1 indexed" in first
    db = sqlite3.connect(f"{base}/index/alice.sqlite")
    report = [t for (t,) in db.execute("SELECT p.text FROM passages p JOIN files f ON f.id = p.file "
                                        "WHERE f.path = 'Documents/rapport.pdf' ORDER BY p.page")]
    db.close()
    yield "a running header and footer are indexed once", report, \
        len(report) == 4 and sum("CONFIDENTIEL" in t for t in report) == 1 and sum("sur 4" in t for t in report) == 1 \
        and all(f"bureau {n}0" in t for n, t in enumerate(report, 1))
    yield "first run takes everything", sorted(files), "Documents/Brouillons/draft.md" in files and "Notes/photo.png" in files
    yield "scan waits for OCR", files.get("Documents/scan.png"), files.get("Documents/scan.png") == "needs_ocr"
    yield "hidden and links left out of the index", sorted(files), \
        not any(".st" in p or ".hidden" in p or "escape" in p or "vieux" in p for p in files)
    yield "prompts sent", EMBEDDED[:1], EMBEDDED and EMBEDDED[0].startswith("title: ") \
        and any(t.startswith("task: search result | query: ") for t in EMBEDDED)
    second = run_indexer(env)
    yield "re-run drops what the filters leave out", second, "alice: 5 dropped, 18 unchanged" in second
    files = statuses(base)
    yield "filtered files leave the index", sorted(files), \
        not any("Brouillons" in p or "Copie" in p or "photo" in p or "alias" in p for p in files)
    os.rename(f"{notes}/courses.md", f"{notes}/liste.md")
    os.remove(f"{docs}/lettre.docx")
    with open(f"{docs}/Factures/Énergie_2024.md", "a") as f:
        f.write("\nrelance")
    with open(f"{docs}/verrou.md", "w") as f:
        f.write("illisible")
    os.chmod(f"{docs}/verrou.md", 0)
    third = run_indexer(env)
    yield "move, delete and edit seen", third, "1 dropped" in third and "1 indexed" in third and "1 moved" in third
    yield "an unreadable file fails alone", third, "1 failed" in third and "ju: 1 unchanged" in third
    os.chmod(f"{docs}/verrou.md", 0o644)
    fourth = run_indexer(env)
    yield "a failed file is retried once fixed", fourth, "1 indexed" in fourth
    files = statuses(base)
    yield "index follows the folders", sorted(files), "Notes/liste.md" in files and "Notes/courses.md" not in files \
        and "Documents/lettre.docx" not in files
    run_indexer({**env, "DOCS_FOLDERS_FILE": f"{base}/moved.json"})
    result = tool("search_documents", query="tomates bob")
    yield "index of a label on another path refused", result, error_with("change of folders")(result)
    run_indexer({**env, "DOCS_FOLDERS_FILE": f"{base}/documents.json"})
    result = tool("search_documents", query="loyer tomates")
    yield "a folder not indexed yet does not stop search", result, ok_with("Documents/bail.pdf")(result) \
        and "Notes/" not in result[1]
    yield "a folder taken away leaves the index", sorted(statuses(base)), not any(p.startswith("Notes/") for p in statuses(base))
    run_indexer(env)
    result = tool("search_documents", query="tomates", path="Notes")
    yield "a folder given back is indexed", result, ok_with("Notes/long.md")(result)
    for name in ("gone.sqlite", "gone.work.sqlite"):
        with open(f"{base}/index/{name}", "w"):
            pass
    run_indexer(env)
    left = sorted(os.listdir(f"{base}/index"))
    yield "a removed user's index goes", left, left == ["alice.sqlite", "alice.status.json", "alice.work.sqlite",
                                                        "ju.sqlite", "ju.status.json", "ju.work.sqlite", "requests", "run.lock"]
    with open(f"{base}/index/alice.status.json") as f:
        status = json.load(f)
    yield "the status file says how the run ended", status, status["result"] == "done" and status["ended"] \
        and status["run"] == {"unchanged": status["files"]["indexed"] + status["files"].get("needs_ocr", 0)
                              + status["files"].get("failed", 0) + status["files"].get("empty", 0)}

    DAV_LOG.clear()
    run_indexer(env)
    yield "unchanged Nextcloud is not listed again", DAV_LOG, DAV_LOG == [("PROPFIND", "0", "/Partage")]
    with open(f"{base}/nextcloud/Partage/compte-rendu.md", "a") as f:
        f.write(" et toiture")
    DAV_LOG.clear()
    out = run_indexer(env)
    listed = [p for m, d, p in DAV_LOG if d == "1"]
    yield "a changed Nextcloud file is indexed", out, "alice: 1 indexed" in out
    yield "only the changed Nextcloud folder is listed", listed, listed == ["/Partage"]
    DAV["down"] = True
    out = run_indexer(env)
    yield "Nextcloud down fails the run", out, "What the index holds of it stays" in out and "index failed for alice" in out
    result = tool("search_documents", query="toiture")
    yield "Nextcloud down keeps its passages", result, ok_with("Cloud/compte-rendu.md")(result)
    result = tool("find_files", query="liste")
    DAV["down"] = False
    yield "find answers from the other folders while Nextcloud is down", result, \
        ok_with("Notes/liste.md", "[Cloud: Nextcloud answered 503.]")(result)
    with open(f"{base}/nextcloud/Partage/compte-rendu.md", "a") as f:
        f.write(" et gouttieres")
    with open(f"{base}/nextcloud/Partage/Sous dossier/nouveau.md", "w") as f:
        f.write("nouveau")
    DAV["fail"] = "/Partage/Sous dossier"
    out = run_indexer(env)
    DAV["fail"] = None
    yield "one unlisted Nextcloud folder fails the run, not its label", out, \
        "Cloud/Sous dossier: Nextcloud did not list it" in out and "1 indexed" in out and "index failed for alice" in out
    result = tool("search_documents", query="jardin partage")
    yield "an unlisted Nextcloud folder keeps its passages", result, ok_with("Cloud/Sous dossier/plan")(result)
    out = run_indexer({**env, "DOCS_NEXTCLOUD_PASSWORD": "wrong"})
    yield "a wrong app password is said", out, "refused the app password" in out
    methods = {m for m, _, _ in DAV_LOG}
    yield "Nextcloud is only read", methods, methods <= {"PROPFIND", "GET"}
    DAV_LOG.clear()
    out = run_indexer({**env, "DOCS_NEXTCLOUD_PASSWORD": "wrong"})
    yield "a refused password is not tried again", (DAV_LOG, out), DAV_LOG == [] and "not tried again" in out
    run_indexer(env)
    yield "a new one is", DAV_LOG[:1], DAV_LOG[:1] == [("PROPFIND", "0", "/Partage")]
    DAV["throttle"] = True
    out = run_indexer(env)
    DAV["throttle"] = False
    yield "a throttled server is said", out, "bruteforce:reset 127.0.0.1" in out

    hour = time.localtime().tm_hour
    write_settings(f"{base}/quiet-settings.json", FILTERS, quiet=f"{hour:02d}-{(hour + 1) % 24:02d}")
    quiet = {**env, "DOCS_SETTINGS_FILE": f"{base}/quiet-settings.json"}
    out = run_indexer(quiet)
    yield "no run in the quiet hours", out, "quiet hours" in out and "alice:" not in out
    open(f"{base}/index/run-now", "w").close()
    out = run_indexer(quiet)
    yield "Run now ignores them, once", out, "alice:" in out and not os.path.exists(f"{base}/index/run-now")
    db = sqlite3.connect(f"{base}/index/alice.sqlite")
    model = dict(db.execute("SELECT key, value FROM meta"))["model"]
    db.close()
    yield "how passages are cut is part of the index's model", model, model == "fake-64/passages-3"
    # Another family: its own passage template, in a throwaway index.
    os.makedirs(f"{base}/jina")
    write_folders(f"{base}/jina.json", {"alice": {"Notes": (f"{base}/notes", "local")}})
    EMBEDDED[:] = []
    run_indexer({**env, "DOCS_EMBED_NAME": "jina-embeddings-v5-text-nano", "DOCS_INDEX_DIR": f"{base}/jina",
                 "DOCS_FOLDERS_FILE": f"{base}/jina.json"})
    yield "each model family gets its own prompt", EMBEDDED[:1], EMBEDDED and all(t.startswith("Document: ") for t in EMBEDDED)
    # A server on a new model searches by keyword until the index is rebuilt for it.
    other = subprocess.Popen([BIN, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             env={**env, "DOCS_EMBED_MODEL": "other-64", "DOCS_EMBED_NAME": "qwen3-embedding",
                                  "DOCS_PORT": "18775"})
    try:
        EMBEDDED[:] = []
        for _ in range(50):
            try:
                found = httpx.post("http://127.0.0.1:18775/mcp", timeout=60, headers={
                    "Accept": "application/json, text/event-stream", "Authorization": f"Bearer {ALICE}"}, json={
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "search_documents", "arguments": {"query": "loyer augmente"}}}).json()
                break
            except httpx.TransportError:
                time.sleep(0.2)
    finally:
        other.terminate()
        other.wait()
    text = found["result"]["content"][0]["text"]
    yield "another model's index is searched by keyword only", text[:200], \
        "rebuilt for a new embedding model" in text and "--- " in text \
        and EMBEDDED and EMBEDDED[0].startswith("Instruct: ")


def remote_checks(base, env):
    """Run now on the desktop, each run on a fresh index against the stand-in Whoopy."""
    def run(name, run_now, key=WHOOPY_KEY, left=-1, skew=False, gone=503):
        index = f"{base}/remote-{name}"
        os.makedirs(index)
        if run_now is not None:
            with open(f"{index}/run-now", "w") as f:
                f.write(run_now)
        WHOOPY.update(calls=0, left=left, skew=skew, gone=gone)
        out = run_indexer({**env, "DOCS_INDEX_DIR": index, "DOCS_REMOTE_URL": f"http://127.0.0.1:{WHOOPY_PORT}",
                           "DOCS_REMOTE_MODEL": "ollama/fake", "DOCS_REMOTE_KEY": key})
        with open(f"{index}/alice.status.json") as f:
            status = json.load(f)
        db = sqlite3.connect(f"{index}/alice.sqlite")
        rows = sorted(db.execute("SELECT f.path, f.status, p.text, p.vector FROM files f "
                                 "LEFT JOIN passages p ON p.file = f.id"))
        db.close()
        return out, status, rows, WHOOPY["calls"]

    _, local, rows, calls = run("timer", None)
    yield "the timer never calls Whoopy", calls, calls == 0 and local["embedded_on"] == ["pi"]
    _, status, _, calls = run("now", "")
    yield "Run now never calls Whoopy", calls, calls == 0 and status["embedded_on"] == ["pi"]
    out, status, remote, calls = run("desktop", "remote")
    yield "Run now on the desktop embeds there, the same vectors", (out[-300:], status), \
        status["embedded_on"] == ["desktop"] and calls > 1 and remote == rows and status["result"] == "done"
    _, status, remote, _ = run("skew", "remote", skew=True)
    yield "vectors that differ keep the run on this server", status, \
        status["embedded_on"] == ["pi"] and "differ" in status["remote_note"] and remote == rows
    _, status, remote, _ = run("key", "remote", key="wrong")
    yield "a refused key keeps the run on this server", status, \
        status["embedded_on"] == ["pi"] and "401" in status["remote_note"] and remote == rows
    _, status, remote, _ = run("drop", "remote", left=3)
    yield "Whoopy gone midway: the run ends on this server, no file failed for it", status, \
        status["embedded_on"] == ["desktop", "pi"] and "stopped answering" in status["remote_note"] \
        and status["result"] == "done" and remote == rows
    _, status, remote, calls = run("asleep", "remote", left=3, gone=500)
    yield "a desktop out of reach midway (500): the same, without halving texts", (status, calls), \
        status["embedded_on"] == ["desktop", "pi"] and status["result"] == "done" and remote == rows and calls == 4
    os.makedirs(f"{base}/remote-early")
    with open(f"{base}/remote-early/run-now", "w") as f:
        f.write("remote")
    run_indexer({**env, "DOCS_INDEX_DIR": f"{base}/remote-early", "DOCS_FOLDERS_FILE": f"{base}/missing.json"})
    yield "a run that fails to start leaves no remote for the timer", os.listdir(f"{base}/remote-early"), \
        not os.path.exists(f"{base}/remote-early/run-now")


def panel_checks():
    """The embedding models the panel offers are the ones docs_embed_model knows,
    each with a family the binary has prompts for. A dot in a choice's key splits it in TOML."""
    with open("cheruby-docs/src/embed.rs") as f:
        source = f.read()
    prefixes = source[source.index("pub fn query_prefix"):source.index("_ => None")]
    prompts = set(re.findall(r'^ +"([a-z0-9-]+)" =>', prefixes, re.M))
    with open("config_panel.toml", "rb") as f:
        offered = set(tomllib.load(f)["main"]["index"]["embed_model"]["choices"])
    common = open("scripts/_common.sh").read()
    known = dict(re.findall(r"^ +([a-z0-9-]+)\) model_source=\S+ model_family=(\S+)", common, re.M))
    yield "the panel offers the models the scripts know", sorted(offered ^ set(known)), offered == set(known)
    yield "every model family has its prompts", sorted(set(known.values()) - prompts), \
        prompts and set(known.values()) <= prompts


def write_checks(base):
    """write_file on disk and in Nextcloud, after the index checks, which count the files."""
    notes = f"{base}/notes"
    result = tool("write_file", path="Notes/idees.md", content="Planter des fèves")
    yield "a file is created", result, ok_with("Created Notes/idees.md")(result)
    result = tool("read_file", path="Notes/idees.md")
    yield "and read back", result, ok_with("Planter des fèves")(result)
    mode = oct(os.stat(f"{notes}/idees.md").st_mode & 0o777)
    yield "it is 660, in its folder's group", mode, mode == "0o660" \
        and os.stat(f"{notes}/idees.md").st_gid == os.stat(notes).st_gid
    os.symlink(f"{base}/secret.md", f"{notes}/lien.md")
    os.link(f"{base}/secret.md", f"{notes}/double.md")
    os.chmod(f"{notes}/long.md", 0o640)
    result = tool("write_file", path="Notes/long.md", content="remplacé")
    with open(f"{notes}/long.md") as f:
        text = f.read()
    yield "a file is replaced in place", (result, text), ok_with("Replaced Notes/long.md")(result) \
        and text == "remplacé" and oct(os.stat(f"{notes}/long.md").st_mode & 0o777) == "0o640"
    result = tool("write_file", path="Notes/Jardin/2026/semis.md", content="mars")
    yield "missing folders are made", result, ok_with("Created Notes/Jardin/2026/semis.md")(result) \
        and oct(os.stat(f"{notes}/Jardin").st_mode & 0o777) == "0o770"
    result = tool("write_file", token=GATEWAY, path="Notes/passerelle.md", content="ok")
    yield "the gateway writes in the folders it opens", result, ok_with("Created")(result)
    for name, path, needle in [("a read-only folder refuses", "Documents/x.md", "Documents is read-only"),
                               ("another user's folder refuses", "Papiers/x.md", "no folder named"),
                               ("only text is written", "Notes/x.pdf", "only text files"),
                               ("the filters refuse what they leave out", "Notes/x.txt", "filters leave this name out"),
                               (".. refused", "Notes/../secret.md", "not allowed"),
                               ("a hidden name refused", "Notes/.cache.md", "not allowed"),
                               ("no write through a link", "Notes/dehors/x.md", "is a file or a link"),
                               ("a link is not replaced", "Notes/lien.md", "is a link"),
                               ("a hard link is not replaced", "Notes/double.md", "hard links"),
                               ("a folder needs a file name", "Notes", "name something inside"),
                               ("a folder is not replaced", "Notes/Jardin", "only text files")]:
        result = tool("write_file", path=path, content="x")
        yield name, result, error_with(needle)(result)
    result = tool("write_file", token=BOB, path="Papiers/x.md", content="x")
    yield "bob's folder is read-only", result, error_with("Papiers is read-only")(result)
    with open(f"{base}/secret.md") as f:
        secret = f.read()
    yield "nothing landed outside", secret, secret == "outside every folder" and not os.path.exists(f"{base}/x.md") \
        and not os.path.exists(f"{base}/docs/x.md")
    result = tool("create_folder", path="Notes/Projets/2027")
    yield "a folder is created with those above it", result, ok_with("Created Notes/Projets/2027/")(result) \
        and os.path.isdir(f"{notes}/Projets/2027") and oct(os.stat(f"{notes}/Projets/2027").st_mode & 0o777) == "0o770"
    result = tool("create_folder", path="Notes/Projets/2027")
    yield "an existing folder is left as it is", result, ok_with("already exists")(result)
    for name, path, needle in [("no folder in a read-only folder", "Documents/Neuf", "Documents is read-only"),
                               ("no folder through a link", "Notes/dehors/x", "is a file or a link"),
                               ("no folder in place of a file", "Notes/idees.md", "is a file or a link"),
                               ("no hidden folder", "Notes/.cache", "not allowed")]:
        result = tool("create_folder", path=path)
        yield name, result, error_with(needle)(result)
    DAV_LOG.clear()
    result = tool("create_folder", path="Cloud/Archives/2026")
    yield "a Nextcloud folder is created", result, ok_with("Created Cloud/Archives/2026/")(result) \
        and os.path.isdir(f"{base}/nextcloud/Partage/Archives/2026")
    result = tool("create_folder", path="Cloud/Vieux/x")
    yield "Nextcloud's filters refuse a folder too", result, error_with("filters leave this name out")(result)
    result = tool("write_file", path="Cloud/Sous dossier/Neuf/note.md", content="dans le nuage")
    yield "a Nextcloud file is created with its folder", result, ok_with("Created Cloud/Sous dossier/Neuf/note.md")(result)
    with open(f"{base}/nextcloud/Partage/Sous dossier/Neuf/note.md") as f:
        text = f.read()
    yield "it is in the account", text, text == "dans le nuage"
    result = tool("write_file", path="Cloud/compte-rendu.md", content="nouveau texte")
    yield "a Nextcloud file is replaced", result, ok_with("Replaced Cloud/compte-rendu.md")(result)
    result = tool("write_file", path="Cloud/Vieux/x.md", content="x")
    yield "Nextcloud's filters refuse too", result, error_with("filters leave this name out")(result)
    methods = {m for m, _, _ in DAV_LOG}
    yield "Nextcloud is never asked to delete or move", methods, methods <= {"PROPFIND", "GET", "PUT", "MKCOL"}
    yield from move_checks(base)


def move_checks(base):
    """move_file on disk and in Nextcloud, after write_checks, which made the files."""
    notes, cloud = f"{base}/notes", f"{base}/nextcloud/Partage"
    os.symlink("long.md", f"{notes}/interne.md")
    result = tool("move_file", source="Notes/idees.md", destination="Notes/Projets/idees.md")
    yield "a file is moved", result, ok_with("Moved Notes/idees.md to Notes/Projets/idees.md")(result) \
        and not os.path.exists(f"{notes}/idees.md") \
        and oct(os.stat(f"{notes}/Projets/idees.md").st_mode & 0o777) == "0o660"
    result = tool("move_file", source="Notes/Jardin", destination="Notes/Potager")
    yield "a folder is renamed", result, ok_with("Moved")(result) \
        and os.path.isfile(f"{notes}/Potager/2026/semis.md")
    for name, source, destination, needle in [
            ("nothing is replaced", "Notes/long.md", "Notes/Projets/idees.md", "already exists"),
            ("a link is not moved", "Notes/interne.md", "Notes/lien2.md", "is a link"),
            ("not into itself", "Notes/Potager", "Notes/Potager/x", "same place or inside it"),
            ("not a folder whose files the filters would judge otherwise", "Notes/Potager", "Notes/Potager.md",
             "filters judge files by their folders"),
            ("not to another folder", "Notes/long.md", "Cloud/long.md", "within one folder"),
            ("not from a read-only folder", "Documents/x.md", "Documents/y.md", "Documents is read-only"),
            ("the new folder must exist", "Notes/long.md", "Notes/Nulle/part.md", "create_folder"),
            ("not to a name the filters leave out", "Notes/long.md", "Notes/x.txt", "filters leave"),
            ("not what the filters leave out", "Notes/x.txt", "Notes/y.md", "not found"),
            ("not a missing file", "Notes/absent.md", "Notes/present.md", "not found"),
            ("not through a link", "Notes/dehors/secret.md", "Notes/x.md", "leads out"),
            (".. refused", "Notes/long.md", "Notes/../long.md", "not allowed"),
            ("not the folder itself", "Notes", "Notes/Autre", "name something inside")]:
        result = tool("move_file", source=source, destination=destination)
        yield f"move: {name}", result, error_with(needle)(result)
    yield "a refused move changes nothing", None, os.path.isfile(f"{notes}/long.md") \
        and os.path.islink(f"{notes}/interne.md") and os.path.isfile(f"{notes}/Projets/idees.md")
    DAV_LOG.clear()
    result = tool("move_file", source="Cloud/Sous dossier/Neuf/note.md", destination="Cloud/Archives/note.md")
    yield "a Nextcloud file is moved", result, ok_with("Moved")(result) \
        and os.path.isfile(f"{cloud}/Archives/note.md") and not os.path.exists(f"{cloud}/Sous dossier/Neuf/note.md")
    result = tool("move_file", source="Cloud/compte-rendu.md", destination="Cloud/Archives/note.md")
    yield "nothing is replaced in Nextcloud", result, error_with("already exists")(result)
    result = tool("move_file", source="Cloud/compte-rendu.md", destination="Cloud/Nulle/part.md")
    yield "Nextcloud's new folder must exist", result, error_with("create_folder")(result)
    os.makedirs(f"{cloud}/De bob")
    with open(f"{cloud}/De bob/sien.md", "w") as f:
        f.write("à bob")
    DAV["bob"] = "Partage/De bob"
    result = tool("move_file", source="Cloud/De bob/sien.md", destination="Cloud/sien.md")
    yield "nothing leaves a share", result, error_with("one account's files")(result) \
        and os.path.isfile(f"{cloud}/De bob/sien.md")
    result = tool("move_file", source="Cloud/compte-rendu.md", destination="Cloud/De bob/cr.md")
    yield "nothing goes into a share", result, error_with("one account's files")(result)
    DAV["bob"] = ""
    DAV["locked"] = {"/Partage/compte-rendu.md"}
    result = tool("write_file", path="Cloud/compte-rendu.md", content="x")
    yield "a locked Nextcloud file says why", result, error_with("Text editor")(result)
    result = tool("move_file", source="Cloud/compte-rendu.md", destination="Cloud/cr.md")
    yield "and is not moved", result, error_with("Text editor")(result)
    DAV["locked"] = set()
    # Back where the checks after these expect them.
    os.remove(f"{notes}/interne.md")
    for source, destination in [("Notes/Projets/idees.md", "Notes/idees.md"), ("Notes/Potager", "Notes/Jardin"),
                                ("Cloud/Archives/note.md", "Cloud/Sous dossier/Neuf/note.md")]:
        result = tool("move_file", source=source, destination=destination)
        yield f"moved back to {destination}", result, ok_with("Moved")(result)
    methods = {m for m, _, _ in DAV_LOG}
    yield "Nextcloud is never asked to delete or copy", methods, methods <= {"PROPFIND", "GET", "PUT", "MKCOL", "MOVE"}


def requests_left(base):
    return [n for n in os.listdir(f"{base}/index/requests") if n.endswith(".json")]


def asked(base, env, **arguments):
    """run_index called as an agent would, with the index run its request
    starts: (the tool's answer, the indexer's output)."""
    answer = {}
    caller = threading.Thread(target=lambda: answer.update(r=tool("run_index", **arguments)))
    caller.start()
    for _ in range(100):
        if requests_left(base):
            break
        time.sleep(0.1)
    out = run_indexer(env)
    caller.join()
    return answer["r"], out


def run_checks(base, env):
    """run_index: a run of files, a run of everything, and what it refuses."""
    tool("write_file", path="Notes/fraise.md", content="Confiture de fraises maison")
    result = tool("search_documents", query="fraises")
    yield "a file just written is not found yet", result, "Notes/fraise.md" not in result[1]
    result, out = asked(base, env, files=["Notes/fraise.md"])
    yield "run_index of a file says how it went", (result, out), \
        ok_with("The index run is done.", "- Notes/fraise.md: indexed")(result) and "alice: 1 indexed" in out
    yield "and does that file only, for its user only", out, "unchanged" not in out and "ju:" not in out
    result = tool("search_documents", query="fraises")
    yield "then search finds it", result, ok_with("--- Notes/fraise.md")(result)
    yield "the request is taken", requests_left(base), requests_left(base) == []
    yield "a run an agent asked for leaves no success-ping", None, not os.path.exists(f"{base}/index/success-ping")
    result, out = asked(base, env)
    yield "run_index of everything answers at once", result, ok_with("all the folders is asked for", "The index holds")(result)
    yield "and its run checks every file of the user", out, "alice:" in out and "unchanged" in out and "ju:" not in out
    open(f"{base}/index/run-now", "w").close()
    with open(f"{base}/index/requests/late.json", "w") as f:
        json.dump({"user": "alice", "files": ["Notes/fraise.md"]}, f)
    out = run_indexer(env)
    yield "a request with Run now gets the full run, answered done", out, \
        "ju:" in out and json.load(open(f"{base}/index/alice.status.json"))["answered"].get("late") == "done"
    yield "Run now leaves success-ping", None, os.path.exists(f"{base}/index/success-ping")
    with open(f"{base}/index/requests/schedule-alice.json", "w") as f:
        json.dump({"user": "alice"}, f)
    out = run_indexer(env)
    yield "the schedule's request is a full run, with success-ping", out, \
        "alice:" in out and "unchanged" in out and os.path.exists(f"{base}/index/success-ping")
    result = tool("run_index")
    yield "not again within 15 minutes", result, error_with("ask again after")(result) and requests_left(base) == []
    for name, files, needle in [(".. refused", ["Notes/../secret.md"], "not allowed"),
                                ("another user's folder refused", ["Papiers/bob.md"], "no folder named"),
                                ("a missing file refused", ["Notes/absent.md"], "not found"),
                                ("a folder refused", ["Notes/Jardin"], "is a folder"),
                                ("at most 50 files", ["Notes/liste.md"] * 51, "at most 50")]:
        result = tool("run_index", files=files)
        yield f"run_index: {name}", result, error_with(needle)(result) and requests_left(base) == []
    for n in range(20):
        open(f"{base}/index/requests/{n}.json", "w").close()
    result = tool("run_index", files=["Notes/liste.md"])
    yield "run_index refused once 20 requests wait", result, error_with("waiting already")(result)
    run_indexer(env)
    yield "unreadable requests are taken too", requests_left(base), requests_left(base) == []
    stale = f"{base}/index/requests/stale.json"
    open(stale, "w").close()
    os.utime(stale, (time.time() - 600, time.time() - 600))
    result = tool("run_index", files=["Notes/liste.md"])
    os.remove(stale)
    yield "a request left untaken is said", result, error_with("not taking them")(result)
    refused = subprocess.run([BIN, "run-index", "Notes/liste.md"], capture_output=True, text=True,
                             env={**env, "DOCS_SETTINGS_FILE": f"{base}/quiet-settings.json"})
    yield "run-index refused in the quiet hours", refused.stderr, \
        refused.returncode != 0 and "quiet hours" in refused.stderr and requests_left(base) == []


def main():
    embedder = ThreadingHTTPServer(("127.0.0.1", EMBED_PORT), FakeEmbedder)
    threading.Thread(target=embedder.serve_forever, daemon=True).start()
    reranker = ThreadingHTTPServer(("127.0.0.1", RERANK_PORT), FakeReranker)
    threading.Thread(target=reranker.serve_forever, daemon=True).start()
    whoopy = ThreadingHTTPServer(("127.0.0.1", WHOOPY_PORT), FakeWhoopy)
    threading.Thread(target=whoopy.serve_forever, daemon=True).start()
    whisper = ThreadingHTTPServer(("127.0.0.1", SPEECH_PORT), FakeWhisper)
    threading.Thread(target=whisper.serve_forever, daemon=True).start()
    nextcloud = ThreadingHTTPServer(("127.0.0.1", DAV_PORT), FakeNextcloud)
    threading.Thread(target=nextcloud.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory() as base:
        env = build(base)
        first = run_indexer({**env, "DOCS_SETTINGS_FILE": f"{base}/open-settings.json"})
        # The unit's UMask.
        server = subprocess.Popen([BIN, "serve"], env={**env, **PAGE_ENV}, umask=0o007,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            for _ in range(50):
                if server.poll() is not None:
                    sys.exit(f"the server stopped: {server.communicate()[1].decode()[-2000:]}")
                try:
                    httpx.get(f"http://127.0.0.1:{PORT}/", timeout=1)
                    break
                except httpx.TransportError:
                    time.sleep(0.2)
            results = [(name, *(lambda r: (r, passes(r)))(run())) for name, run, passes in CHECKS]
            results += list(index_checks(base, env, first))
            results += list(remote_checks(base, env))
            results += list(write_checks(base))
            results += list(run_checks(base, env))
            results += list(speech_checks(base))
            results += list(panel_checks())
            results += list(page_checks(base))
            results += list(settings_checks(base, env))
            refused = subprocess.run([BIN, "serve"], capture_output=True, text=True,
                                     env={**env, "DOCS_GATEWAY_FOLDERS": "Notes:Papiers", "DOCS_PORT": "18772"})
            results.append(("gateway refused a folder its user lacks", refused.stderr[-200:],
                            refused.returncode != 0 and "does not have" in refused.stderr))
        finally:
            server.terminate()
            err = server.communicate()[1].decode()
        failed = 0
        for name, result, good in results:
            failed += not good
            print(f"{'PASS' if good else 'FAIL'}  {name}" + ("" if good else f"\n      got {result!r:.300}"))
        if failed:
            print(err[-2000:])
        print(f"{len(results) - failed}/{len(results)} passed")
        sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
