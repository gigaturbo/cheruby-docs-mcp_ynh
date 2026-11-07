"""Offline check of the tool server. Run from the project root:

    .venv/bin/python tests/check_server.py

Builds two users' folders in a temporary directory, starts the server on a
loopback port, then calls each tool. Needs the package's dependencies only.
Prints one line per check and exits 1 if any fails.
"""

import base64
import os
import subprocess
import sys
import tempfile
import time
import zipfile

import httpx

ALICE, BOB = "b" * 40, "j" * 40
PORT = 18770
URL = f"http://127.0.0.1:{PORT}/mcp"
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


def build(base):
    """alice: Documents and Notes. bob: Papiers. Returns the env for the server."""
    docs, notes, papiers = (os.path.join(base, d) for d in ("docs", "notes", "papiers"))
    os.makedirs(os.path.join(docs, "Factures"))
    os.makedirs(os.path.join(docs, ".stversions"))
    os.makedirs(notes)
    os.makedirs(papiers)
    files = {
        f"{docs}/Factures/Énergie_2024.md": "Facture EDF\n" + "x" * 30_000,
        f"{docs}/.stversions/old.md": "hidden",
        f"{docs}/.hidden.md": "hidden",
        f"{notes}/courses.md": "pain, lait",
        f"{papiers}/bob.md": "bob only",
        f"{base}/secret.md": "outside every folder",
    }
    for path, text in files.items():
        with open(path, "w") as f:
            f.write(text)
    with open(f"{docs}/bail.pdf", "wb") as f:
        f.write(tiny_pdf("Loyer augmente de 3 pourcent"))
    with open(f"{docs}/scan.png", "wb") as f:
        f.write(PNG)
    with zipfile.ZipFile(f"{docs}/lettre.docx", "w") as z:
        z.writestr("word/document.xml",
                   '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
                   "<w:body><w:p><w:r><w:t>Madame, </w:t></w:r><w:r><w:t>Monsieur</w:t></w:r></w:p>"
                   "<w:p><w:r><w:t>Objet : préavis</w:t></w:r></w:p></w:body></w:document>")
    os.symlink(f"{base}/secret.md", f"{docs}/escape.md")
    return {
        "PATH": os.environ.get("PATH", ""),
        "DOCS_PORT": str(PORT),
        "DOCS_TOKEN_alice": ALICE,
        "DOCS_FOLDERS_alice": f"Documents={docs}:Notes={notes}",
        "DOCS_TOKEN_ju": BOB,
        "DOCS_FOLDERS_ju": f"Papiers={papiers}",
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
    ("three tools listed", lambda: call("tools/list"), lambda r: r[1] == "list_folder find_files read_file"),
    ("alice sees her two folders", lambda: tool("list_folder"), lambda r: r == ("ok", "dir  Documents/\ndir  Notes/")),
    ("bob sees only his", lambda: tool("list_folder", token=BOB), lambda r: r == ("ok", "dir  Papiers/")),
    ("bob cannot reach alice's", lambda: tool("read_file", token=BOB, path="Notes/courses.md"), error_with("no folder named")),
    ("alice cannot reach bob's", lambda: tool("read_file", path="Papiers/bob.md"), error_with("no folder named")),
    ("folder listed, hidden left out", lambda: tool("list_folder", path="Documents"),
     lambda r: r[0] == "ok" and "Documents/Factures/" in r[1] and "bail.pdf" in r[1] and ".st" not in r[1] and ".hidden" not in r[1]),
    (".. refused", lambda: tool("read_file", path="Documents/../secret.md"), error_with("not allowed")),
    ("hidden refused", lambda: tool("read_file", path="Documents/.stversions/old.md"), error_with("not allowed")),
    ("link out refused", lambda: tool("read_file", path="Documents/escape.md"), error_with("leads out")),
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
]


def main():
    with tempfile.TemporaryDirectory() as base:
        env = build(base)
        server = subprocess.Popen([sys.executable, "-I", "conf/cheruby_docs_mcp.py"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            for _ in range(50):
                try:
                    httpx.get(f"http://127.0.0.1:{PORT}/", timeout=1)
                    break
                except httpx.TransportError:
                    time.sleep(0.2)
            failed = 0
            for name, run, passes in CHECKS:
                result = run()
                good = passes(result)
                failed += not good
                print(f"{'PASS' if good else 'FAIL'}  {name}" + ("" if good else f"\n      got {result!r:.300}"))
        finally:
            server.terminate()
            err = server.communicate()[1].decode()
        if failed:
            print(err[-2000:])
        print(f"{len(CHECKS) - failed}/{len(CHECKS)} passed")
        sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
