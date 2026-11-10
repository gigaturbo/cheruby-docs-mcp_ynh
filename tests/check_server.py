"""Offline check of the tool server and the indexer. Run from the project root:

    .venv/bin/python tests/check_server.py

Builds two users' folders in a temporary directory, indexes them without
filters against a stand-in embedding server, starts the tool server with
filters on a loopback port, then calls each tool and re-runs the indexer
with the filters, then after a move, an edit and a deletion.
The stand-in hashes words into vectors: it checks the wiring, not the quality
of the search. Needs the package's dependencies only.
Prints one line per check and exits 1 if any fails.
"""

import base64
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx

ALICE, BOB, GATEWAY = "b" * 40, "j" * 40, "g" * 40
PORT = 18770
EMBED_PORT = 18771
URL = f"http://127.0.0.1:{PORT}/mcp"
PYTHON = [sys.executable, "-E", "-s", "-B"]
EMBEDDED = []
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")


def tiny_pdf(text):
    """A one-page PDF whose page shows text, with a correct cross-reference table."""
    stream = f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
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

    A text containing "panne" gets a 500, to see the server fall back on keywords.
    """

    def do_GET(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        texts = json.loads(self.rfile.read(int(self.headers["Content-Length"])))["input"]
        EMBEDDED.extend(texts)
        if any("panne" in t for t in texts):
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


def write_folders(path, folders):
    """The folders file: {user: {label: (real path, only, skip)}}."""
    with open(path, "w") as f:
        json.dump({user: {label: {"path": p, "only": only, "skip": skip} for label, (p, only, skip) in entries.items()}
                   for user, entries in folders.items()}, f)


def build(base):
    """alice: Documents and Notes, filtered. bob: Papiers. Returns the env for the
    server, and the folders without filters for the first index run."""
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
                   "<w:body><w:p><w:r><w:t>Madame, </w:t></w:r><w:r><w:t>Monsieur</w:t></w:r></w:p>"
                   "<w:p><w:r><w:t>Objet : préavis</w:t></w:r></w:p></w:body></w:document>")
    os.symlink(f"{base}/secret.md", f"{docs}/escape.md")
    os.symlink(".stversions/old.md", f"{docs}/vieux.md")
    os.symlink("Brouillons", f"{docs}/Pub")
    os.symlink("Brouillons/draft.md", f"{docs}/alias.md")
    os.utime(f"{notes}/courses.md", (946_000_000, 946_000_000))  # 1999-12-24
    os.makedirs(f"{base}/index")
    folders = {"alice": {"Documents": (docs, [], ["brouillons", "/Documents/*.tmp.md"]),
                         "Notes": (notes, ["*.md"], [])},
               "ju": {"Papiers": (papiers, [], [])}}
    write_folders(f"{base}/folders.json", folders)
    write_folders(f"{base}/open.json", {user: {label: (p, [], []) for label, (p, _, _) in entries.items()}
                                        for user, entries in folders.items()})
    write_folders(f"{base}/documents.json", {"alice": {"Documents": folders["alice"]["Documents"]},
                                             "ju": folders["ju"]})
    return {
        "PATH": os.environ.get("PATH", ""),
        "DOCS_PORT": str(PORT),
        "DOCS_INDEX_DIR": f"{base}/index",
        "DOCS_EMBED_URL": f"http://127.0.0.1:{EMBED_PORT}",
        "DOCS_EMBED_MODEL": "fake-64",
        "DOCS_FOLDERS_FILE": f"{base}/folders.json",
        "DOCS_TOKEN_alice": ALICE,
        "DOCS_TOKEN_ju": BOB,
        "DOCS_GATEWAY_TOKEN": GATEWAY,
        "DOCS_GATEWAY_USER": "alice",
        "DOCS_GATEWAY_FOLDERS": "Notes",
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
    ("four tools listed", lambda: call("tools/list"), lambda r: r[1] == "list_folder find_files read_file search_documents"),
    ("alice sees her two folders", lambda: tool("list_folder"), lambda r: r == ("ok", "dir  Documents/\ndir  Notes/")),
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
    ("nor the folder link", lambda: tool("find_files", query="pub"), lambda r: r == ("ok", "Nothing matches 'pub'.")),
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
    ("one passage per page", lambda: tool("search_documents", query="jardin tomates", limit=10),
     lambda r: r[0] == "ok" and r[1].count("--- Notes/long.md ") == 1),
    ("search leaves hidden out", lambda: tool("search_documents", query="hidden", limit=10),
     lambda r: r[0] == "ok" and ".st" not in r[1] and ".hidden" not in r[1]),
    ("search refuses ..", lambda: tool("search_documents", query="x", path="Notes/.."), error_with("not allowed")),
    ("search bad date refused", lambda: tool("search_documents", query="x", after="hier"), error_with("not a date")),
    ("bob's search sees only his", lambda: tool("search_documents", token=BOB, query="loyer bob"),
     lambda r: r[0] == "ok" and "Papiers/bob.md" in r[1] and "Documents" not in r[1]),
    ("gateway sees only the folders it opens", lambda: tool("list_folder", token=GATEWAY), lambda r: r == ("ok", "dir  Notes/")),
    ("gateway cannot read the others", lambda: tool("read_file", token=GATEWAY, path="Documents/bail.pdf"),
     error_with("no folder named")),
    ("gateway search uses the user's index, its folders only", lambda: tool("search_documents", token=GATEWAY, query="loyer tomates"),
     lambda r: r[0] == "ok" and "Notes/long.md" in r[1] and "Documents/" not in r[1]),
    ("gateway search refuses the others", lambda: tool("search_documents", token=GATEWAY, query="loyer", path="Documents"),
     error_with("no folder named")),
    ("search falls back on keywords", lambda: tool("search_documents", query="panne loyer"),
     ok_with("keyword matches only", "Documents/bail.pdf")),
]


def run_indexer(env):
    out = subprocess.run([*PYTHON, "conf/docs_index.py"], env=env, capture_output=True, text=True, timeout=120)
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
    yield "first run takes everything", sorted(files), "Documents/Brouillons/draft.md" in files and "Notes/photo.png" in files
    yield "scan waits for OCR", files.get("Documents/scan.png"), files.get("Documents/scan.png") == "needs_ocr"
    yield "hidden and links left out of the index", sorted(files), \
        not any(".st" in p or ".hidden" in p or "escape" in p or "vieux" in p for p in files)
    yield "prompts sent", EMBEDDED[:1], EMBEDDED and EMBEDDED[0].startswith("title: ") \
        and any(t.startswith("task: search result | query: ") for t in EMBEDDED)
    second = run_indexer(env)
    yield "re-run drops what the filters leave out", second, "alice: 4 dropped, 6 unchanged" in second
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
    run_indexer({**env, "DOCS_FOLDERS_FILE": f"{base}/documents.json"})
    result = tool("search_documents", query="tomates")
    yield "index of other folders refused", result, error_with("change of folders")(result)
    yield "a folder taken away leaves the index", sorted(statuses(base)), not any(p.startswith("Notes/") for p in statuses(base))
    run_indexer(env)
    result = tool("search_documents", query="tomates", path="Notes")
    yield "a folder given back is indexed", result, ok_with("Notes/long.md")(result)
    for name in ("gone.sqlite", "gone.work.sqlite"):
        with open(f"{base}/index/{name}", "w"):
            pass
    run_indexer(env)
    left = sorted(os.listdir(f"{base}/index"))
    yield "a removed user's index goes", left, left == ["alice.sqlite", "alice.work.sqlite", "ju.sqlite", "ju.work.sqlite"]


def main():
    embedder = ThreadingHTTPServer(("127.0.0.1", EMBED_PORT), FakeEmbedder)
    threading.Thread(target=embedder.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory() as base:
        env = build(base)
        first = run_indexer({**env, "DOCS_FOLDERS_FILE": f"{base}/open.json"})
        server = subprocess.Popen([*PYTHON, "conf/cheruby_docs_mcp.py"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            for _ in range(50):
                try:
                    httpx.get(f"http://127.0.0.1:{PORT}/", timeout=1)
                    break
                except httpx.TransportError:
                    time.sleep(0.2)
            results = [(name, *(lambda r: (r, passes(r)))(run())) for name, run, passes in CHECKS]
            results += list(index_checks(base, env, first))
            refused = subprocess.run([*PYTHON, "conf/cheruby_docs_mcp.py"], capture_output=True, text=True,
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
