"""What the tool server and the indexer both know about the users' folders.

The folders file and its filters, which names stay hidden, and how text
comes out of a file. Nothing here writes. Errors a user should see are ValueError.
"""

import json
import logging
import os
import sys
import zipfile
from fnmatch import fnmatchcase
from xml.etree import ElementTree

from pypdf import PdfReader

# pypdf warns on every odd font; the journal needs its errors only.
logging.getLogger("pypdf").setLevel(logging.ERROR)

MAX_XML_BYTES = 20_000_000
TEXT_SUFFIXES = {".md", ".txt", ".csv", ".json", ".vcf", ".ics", ".xml", ".html", ".htm", ".yml", ".yaml", ".log"}
OFFICE_MEMBERS = {".docx": "word/document.xml", ".odt": "content.xml"}
IMAGE_FORMATS = {".jpg": "jpeg", ".jpeg": "jpeg", ".png": "png", ".webp": "webp", ".gif": "gif"}


def load_folders(path):
    """({user: {label: real path}}, {user: {label: (only, skip)}}) from the folders file.

    The file is {user: {label: {"path": ..., "only": [...], "skip": [...]}}},
    written by the package from the config panel's sources.
    """
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    folders, rules = {}, {}
    for user, entries in data.items():
        folders[user], rules[user] = {}, {}
        for label, entry in entries.items():
            if not label or "/" in label or hidden(label):
                sys.exit(f"{path}: bad folder label {label!r}.")
            folders[user][label] = os.path.realpath(entry["path"])
            rules[user][label] = tuple([p.lower().strip("/") for p in entry.get(key, []) if p.strip("/")]
                                       for key in ("only", "skip"))
    return folders, rules


def matches(pattern, parts):
    """Whether a pattern matches a tool path, given split at /, or a folder above it.

    Without a /, it matches any one name (*.pdf, Archives); with one, the path
    from its label on (Documents/Old, Documents/*/2019). * crosses /.
    """
    if "/" in pattern:
        return any(fnmatchcase("/".join(parts[:n]), pattern) for n in range(1, len(parts) + 1))
    return any(fnmatchcase(p, pattern) for p in parts)


def kept(rules, shown, is_dir=False):
    """Whether its folder's filters let a tool path through, ignoring case.

    Skip takes out what it matches and everything below. Only, when set, keeps
    just the files it matches or that lie in a folder it matches; folders pass
    it, since they may hold such files.
    """
    only, skip = rules.get(shown.split("/", 1)[0], ((), ()))
    parts = shown.lower().split("/")
    if any(matches(p, parts) for p in skip):
        return False
    return is_dir or not only or any(matches(p, parts) for p in only)


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


def kept_target(rules, label, root, path, is_dir=False):
    """Whether path, once links are followed, stays in root out of hidden names
    and is kept by the filters: a link cannot show a filtered file under its
    own name."""
    real = os.path.realpath(path)
    if not inside(real, root):
        return False
    rel = os.path.relpath(real, root)
    return kept(rules, label if rel == "." else f"{label}/{rel}", is_dir)


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
