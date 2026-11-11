"""What the tool server and the indexer both know about the users' folders.

The folders file and its filters, which names stay hidden, how a folder on
disk or in Nextcloud is listed and read, and how text comes out of a file.
Nothing here writes. Errors a user should see are ValueError; Nextcloud's
are NextcloudError, an OSError.
"""

import io
import json
import logging
import os
import stat
import sys
import urllib.parse
import zipfile
from email.utils import parsedate_to_datetime
from fnmatch import fnmatchcase
from typing import NamedTuple
from xml.etree import ElementTree

import httpx2
from pypdf import PdfReader

# pypdf warns on every odd font; the journal needs its errors only.
logging.getLogger("pypdf").setLevel(logging.ERROR)

MAX_XML_BYTES = 20_000_000
TEXT_SUFFIXES = {".md", ".txt", ".csv", ".json", ".vcf", ".ics", ".xml", ".html", ".htm", ".yml", ".yaml", ".log"}
OFFICE_MEMBERS = {".docx": "word/document.xml", ".odt": "content.xml"}
IMAGE_FORMATS = {".jpg": "jpeg", ".jpeg": "jpeg", ".png": "png", ".webp": "webp", ".gif": "gif"}
# A folder in the user's Nextcloud is "nextcloud:<path in the account>" where
# one on disk is its real path.
NEXTCLOUD = "nextcloud:"


def needs_nextcloud(folders, nextcloud):
    """Exits when a folder is in Nextcloud and no account is set."""
    if nextcloud is None and any(r.startswith(NEXTCLOUD) for f in folders.values() for r in f.values()):
        sys.exit("A folder is in Nextcloud, and DOCS_NEXTCLOUD_URL is not set.")


def load_folders(path):
    """({user: {label: root}}, {user: {label: (only, skip)}}) from the folders file.

    The file is {user: {label: {"path": ..., "only": [...], "skip": [...]}}},
    written by the package from the config panel's sources; a Nextcloud
    folder has "nextcloud": <path in the account> in place of "path". A root
    is a real path, or NEXTCLOUD and the account path ("" for all of it).
    """
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    folders, rules = {}, {}
    for user, entries in data.items():
        folders[user], rules[user] = {}, {}
        for label, entry in entries.items():
            if not label or "/" in label or hidden(label):
                sys.exit(f"{path}: bad folder label {label!r}.")
            if "nextcloud" in entry:
                folders[user][label] = NEXTCLOUD + "/".join(p for p in entry["nextcloud"].split("/") if p)
            else:
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


def office_text(source, suffix):
    """Paragraphs of a docx or odt, from a path or a binary file, one per line."""
    member = OFFICE_MEMBERS[suffix]
    with zipfile.ZipFile(source) as z:
        if z.getinfo(member).file_size > MAX_XML_BYTES:
            raise ValueError("the document's text is too large to read.")
        root = ElementTree.fromstring(z.read(member))
    paragraphs = (("".join(el.itertext())).strip() for el in root.iter()
                  if el.tag.rsplit("}", 1)[-1] in ("p", "h"))
    return "\n".join(p for p in paragraphs if p)


def open_pdf(source):
    """A PDF from a path or a binary file."""
    reader = PdfReader(source)
    if reader.is_encrypted and not reader.decrypt(""):
        raise ValueError("the PDF is password-protected.")
    return reader


class NextcloudError(OSError):
    """Nextcloud did not answer, or refused the app password."""


class NextcloudRefused(NextcloudError):
    """Nextcloud refused the app password: nothing else will answer either."""


class Entry(NamedTuple):
    """One file or folder of a Nextcloud account, by its path in it: "" is the account."""
    path: str
    is_dir: bool
    size: int
    mtime_ns: int
    etag: str


PROPFIND = (b'<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/>'
            b'<d:getcontentlength/><d:getlastmodified/><d:getetag/></d:prop></d:propfind>')


class Nextcloud:
    """One Nextcloud account over WebDAV. Read-only: it sends PROPFIND and GET, nothing else.

    url is the account's WebDAV root as reached, such as
    https://127.0.0.1/remote.php/dav/files/<user>; host, when set, is the name
    Nextcloud answers to, sent as Host and checked on its certificate.
    """

    def __init__(self, url, host, user, password):
        self.base = urllib.parse.unquote(urllib.parse.urlsplit(url).path).rstrip("/")
        self.client = httpx2.Client(base_url=url.rstrip("/") + "/", auth=(user, password), timeout=60,
                                    headers={"Host": host} if host else {})
        self.extensions = {"sni_hostname": host} if host else {}

    @classmethod
    def from_env(cls, environ):
        """The account in DOCS_NEXTCLOUD_URL, _HOST, _USER and _PASSWORD, or None."""
        if not environ.get("DOCS_NEXTCLOUD_URL"):
            return None
        return cls(environ["DOCS_NEXTCLOUD_URL"], environ.get("DOCS_NEXTCLOUD_HOST", ""),
                   environ["DOCS_NEXTCLOUD_USER"], environ["DOCS_NEXTCLOUD_PASSWORD"])

    def list(self, path, depth):
        """[Entry] of path then, at depth 1, what it holds; None when it does not exist."""
        quoted = "/".join(urllib.parse.quote(p, safe="") for p in path.split("/") if p)
        try:
            resp = self.client.request("PROPFIND", quoted, content=PROPFIND, extensions=self.extensions,
                                       headers={"Depth": str(depth), "Content-Type": "application/xml"})
        except httpx2.HTTPError as e:
            raise NextcloudError(f"Nextcloud did not answer: {e}") from None
        if resp.status_code == 404:
            return None
        if resp.status_code == 401:
            raise NextcloudRefused("Nextcloud refused the app password.")
        if resp.status_code != 207:
            raise NextcloudError(f"Nextcloud answered {resp.status_code}.")
        try:
            responses = list(ElementTree.fromstring(resp.content).iter("{DAV:}response"))
        except ElementTree.ParseError:
            raise NextcloudError("Nextcloud's answer could not be read.") from None
        entries = {}
        for response in responses:
            href = urllib.parse.unquote(urllib.parse.urlsplit(response.findtext("{DAV:}href", "")).path)
            if href.rstrip("/") != self.base and not href.startswith(self.base + "/"):
                continue
            found = {el.tag.rsplit("}", 1)[-1]: el for el in response.iter()}
            modified = found.get("getlastmodified")
            entries[href[len(self.base):].strip("/")] = Entry(
                href[len(self.base):].strip("/"), "collection" in found,
                int((found["getcontentlength"].text or 0) if "getcontentlength" in found else 0),
                int(parsedate_to_datetime(modified.text).timestamp() * 1e9) if modified is not None and modified.text else 0,
                (found["getetag"].text or "") if "getetag" in found else "")
        here = entries.pop(path.strip("/"), None)
        if here is None:
            raise NextcloudError(f"Nextcloud did not describe {path or 'the account'}.")
        return [here, *sorted(entries.values())]

    def get(self, path, limit):
        """The bytes of a file, refused past limit."""
        quoted = "/".join(urllib.parse.quote(p, safe="") for p in path.split("/") if p)
        data = bytearray()
        try:
            with self.client.stream("GET", quoted, extensions=self.extensions) as resp:
                if resp.status_code == 404:
                    raise FileNotFoundError(path)
                if resp.status_code != 200:
                    raise NextcloudError(f"Nextcloud answered {resp.status_code}.")
                for block in resp.iter_bytes():
                    data += block
                    if len(data) > limit:
                        raise ValueError(f"the file is larger than {limit // 1_000_000} MB, too large to fetch from Nextcloud.")
        except httpx2.HTTPError as e:
            raise NextcloudError(f"Nextcloud did not answer: {e}") from None
        return bytes(data)


class Item(NamedTuple):
    """A file or folder as a token sees it."""
    shown: str        # its tool path, Documents/Factures/x.pdf
    is_dir: bool
    size: int
    mtime_ns: int
    ctime_ns: int     # Nextcloud has none: its mtime
    where: str        # the real path, or the path in the Nextcloud account
    nextcloud: bool
    link: bool = False
    etag: str = ""


class Folders:
    """One token's folders, {label: root}, with their filters: what it can see
    in them, and how to read it. Every listing goes through here, so hidden
    names, links out and what the filters leave out are dropped one way only.

    cache, for the indexer: {account path: [etag, [Entry fields]]} of
    Nextcloud folders already listed. A folder whose ETag has not changed
    holds the same things, so it is not asked again. Every folder listed
    goes to self.listed, to be stored for the next run.
    """

    def __init__(self, roots, rules, nextcloud, cache=None):
        self.roots, self.rules, self.nextcloud = roots, rules, nextcloud
        self.cache, self.listed = cache, {}
        # Tool paths of the Nextcloud folders a walk could not list.
        self.unlisted = []

    def item(self, label, parts):
        """The Item at label/parts, None when it is missing or filtered out.

        Raises ValueError for a path that leads out of its folder or into a
        hidden one. parts holds no "..", no hidden name: the caller checks.
        """
        root, shown = self.roots[label], "/".join([label, *parts])
        if root.startswith(NEXTCLOUD):
            where = "/".join(p for p in [root[len(NEXTCLOUD):], *parts] if p)
            entries = self.nextcloud.list(where, 0)
            if entries is None or not kept(self.rules, shown, entries[0].is_dir):
                return None
            e = entries[0]
            return Item(shown, e.is_dir, e.size, e.mtime_ns, e.mtime_ns, where, True, etag=e.etag)
        real = os.path.realpath(os.path.join(root, *parts))
        if not inside(real, root):
            raise ValueError(f"leads out of {label} or into a hidden folder.")
        # Left out by a filter, under its name or where it leads, reads as missing.
        try:
            st = os.stat(real)
        except OSError:
            return None
        is_dir = stat.S_ISDIR(st.st_mode)
        if not kept(self.rules, shown, is_dir) or not kept_target(self.rules, label, root, real, is_dir):
            return None
        return Item(shown, is_dir, st.st_size, st.st_mtime_ns, st.st_ctime_ns, real, False)

    def children(self, folder):
        """The Items in a folder Item, sorted by name. Raises OSError when it cannot be listed."""
        label = folder.shown.split("/", 1)[0]
        if folder.nextcloud:
            cached = (self.cache or {}).get(folder.where)
            if cached and folder.etag and cached[0] == folder.etag:
                entries = [Entry(*e) for e in cached[1]]
            else:
                entries = self.nextcloud.list(folder.where, 1)
                if entries is None:
                    raise FileNotFoundError(folder.shown)
                entries = entries[1:]
            self.listed[folder.where] = [folder.etag, [list(e) for e in entries]]
            items = []
            for e in entries:
                name = e.path.rsplit("/", 1)[-1]
                shown = f"{folder.shown}/{name}"
                if not hidden(name) and kept(self.rules, shown, e.is_dir):
                    items.append(Item(shown, e.is_dir, e.size, e.mtime_ns, e.mtime_ns, e.path, True, etag=e.etag))
            return items
        root, items = self.roots[label], []
        with os.scandir(folder.where) as scan:
            for e in scan:
                shown = f"{folder.shown}/{e.name}"
                if hidden(e.name):
                    continue
                try:
                    st = e.stat()
                except OSError:  # a broken link
                    continue
                is_dir = stat.S_ISDIR(st.st_mode)
                if not is_dir and not stat.S_ISREG(st.st_mode):
                    continue
                link = e.is_symlink()
                # A link is judged by where it leads too.
                if not kept(self.rules, shown, is_dir) or (link and not kept_target(self.rules, label, root, e.path, is_dir)):
                    continue
                items.append(Item(shown, is_dir, st.st_size, st.st_mtime_ns, st.st_ctime_ns, e.path, False, link))
        return sorted(items, key=lambda i: i.shown)

    def walk(self, folder):
        """Every Item below a folder, depth first. Links to folders are not
        followed. A folder that cannot be listed is passed over, a Nextcloud
        one noted in self.unlisted; a refused app password propagates."""
        try:
            items = self.children(folder)
        except NextcloudRefused:
            raise
        except NextcloudError:
            self.unlisted.append(folder.shown)
            return
        except OSError:
            return
        for item in items:
            yield item
            if item.is_dir and not item.link:
                yield from self.walk(item)

    def open(self, item, limit):
        """A binary file to read the Item from. A Nextcloud file is fetched whole, up to limit."""
        if item.nextcloud:
            return io.BytesIO(self.nextcloud.get(item.where, limit))
        return open(item.where, "rb")
