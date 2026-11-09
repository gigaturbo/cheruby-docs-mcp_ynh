"""What the tool server and the indexer both know about the users' folders.

The folder settings, which names stay hidden, and how text comes out of a
file. Nothing here writes. Errors a user should see are ValueError.
"""

import logging
import os
import sys
import zipfile
from xml.etree import ElementTree

from pypdf import PdfReader

# pypdf warns on every odd font; the journal needs its errors only.
logging.getLogger("pypdf").setLevel(logging.ERROR)

MAX_XML_BYTES = 20_000_000
TEXT_SUFFIXES = {".md", ".txt", ".csv", ".json", ".vcf", ".ics", ".xml", ".html", ".htm", ".yml", ".yaml", ".log"}
OFFICE_MEMBERS = {".docx": "word/document.xml", ".odt": "content.xml"}
IMAGE_FORMATS = {".jpg": "jpeg", ".jpeg": "jpeg", ".png": "png", ".webp": "webp", ".gif": "gif"}


def load_folders(environ):
    """{user: {label: real path}} from DOCS_FOLDERS_<user>, "<label>=<path>" joined by ":"."""
    users = {}
    for key, value in environ.items():
        if not key.startswith("DOCS_FOLDERS_"):
            continue
        folders = {}
        for entry in filter(None, value.split(":")):
            label, sep, path = entry.partition("=")
            if not sep or not label or "/" in label or label.startswith(".") or label in folders:
                sys.exit(f"{key}: bad or repeated entry {entry!r}.")
            folders[label] = os.path.realpath(path)
        users[key.removeprefix("DOCS_FOLDERS_")] = folders
    return users


def hidden(name):
    """Syncthing's .stfolder and .stversions, and every other dot name."""
    return name.startswith(".")


def inside(real, root):
    """Whether a resolved path lies in root without passing a hidden name, so a
    link to .stversions/ inside the folder is refused like the name itself."""
    if real == root:
        return True
    if not real.startswith(root + os.sep):
        return False
    return not any(hidden(p) for p in os.path.relpath(real, root).split(os.sep))


def office_text(real):
    """Paragraphs of a docx or odt, one per line."""
    member = OFFICE_MEMBERS[os.path.splitext(real)[1].lower()]
    with zipfile.ZipFile(real) as z:
        if z.getinfo(member).file_size > MAX_XML_BYTES:
            raise ValueError("the document's text is too large to read.")
        root = ElementTree.fromstring(z.read(member))
    paragraphs = (("".join(el.itertext())).strip() for el in root.iter()
                  if el.tag.rsplit("}", 1)[-1] in ("p", "h"))
    return "\n".join(p for p in paragraphs if p)


def open_pdf(real):
    reader = PdfReader(real)
    if reader.is_encrypted and not reader.decrypt(""):
        raise ValueError("the PDF is password-protected.")
    return reader
