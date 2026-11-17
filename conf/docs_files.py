"""What the tool server and the indexer both know about the users' folders.

The folders file and its filters, which names stay hidden, how a folder on
disk or in Nextcloud is listed and read, and how text comes out of a file.
Only Folders.write writes, for the tool server, in the folders marked
writable. Errors a user should see are ValueError; Nextcloud's are
NextcloudError, an OSError.
"""

import errno
import hashlib
import io
import json
import logging
import os
import re
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
# A write never leaves the disk, shared with everything else, fuller than this.
MIN_FREE_BYTES = 2_000_000_000
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
    """({user: {label: root}}, {user: {label: (only, skip)}}, {user: {writable labels}})
    from the folders file.

    The file is {user: {label: {"path": ..., "only": [...], "skip": [...]}}},
    written by the package from the config panel's sources; a Nextcloud
    folder has "nextcloud": <path in the account> in place of "path", and a
    writable one "writable": true. A root is a real path, or NEXTCLOUD and
    the account path ("" for all of it).
    """
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    folders, rules, writable = {}, {}, {}
    for user, entries in data.items():
        folders[user], rules[user], writable[user] = {}, {}, set()
        for label, entry in entries.items():
            if not label or "/" in label or hidden(label):
                sys.exit(f"{path}: bad folder label {label!r}.")
            if "nextcloud" in entry:
                folders[user][label] = NEXTCLOUD + "/".join(p for p in entry["nextcloud"].split("/") if p)
            else:
                folders[user][label] = os.path.realpath(entry["path"])
            rules[user][label] = tuple([p.lower().strip("/") for p in entry.get(key, []) if p.strip("/")]
                                       for key in ("only", "skip"))
            if entry.get("writable"):
                writable[user].add(label)
    return folders, rules, writable


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


WORD = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
ODF_TEXT = "{urn:oasis:names:tc:opendocument:xmlns:text:1.0}"
# Word names its heading styles in the document's language: Heading1, Titre1.
HEADING_STYLE = re.compile(r"(?i)^(?:heading|titre)\s*([1-6])$")


def heading_level(el):
    """1 to 6 for a heading paragraph of a docx or odt, else 0."""
    if el.tag == ODF_TEXT + "h":
        level = el.get(ODF_TEXT + "outline-level", "1")
        return min(max(int(level), 1), 6) if level.isdigit() else 1
    style = el.find(f"{WORD}pPr/{WORD}pStyle")
    match = style is not None and HEADING_STYLE.match(style.get(WORD + "val", ""))
    return int(match[1]) if match else 0


def office_text(source, suffix):
    """Paragraphs of a docx or odt, from a path or a binary file, one per line,
    headings as Markdown "#" lines."""
    member = OFFICE_MEMBERS[suffix]
    with zipfile.ZipFile(source) as z:
        if z.getinfo(member).file_size > MAX_XML_BYTES:
            raise ValueError("the document's text is too large to read.")
        root = ElementTree.fromstring(z.read(member))
    lines = []
    for el in root.iter():
        if el.tag.rsplit("}", 1)[-1] in ("p", "h"):
            text = "".join(el.itertext()).strip()
            if text:
                level = heading_level(el)
                lines.append(f"{'#' * level} {text}" if level else text)
    return "\n".join(lines)


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


def quote(path):
    """A path in the account as a URL path, relative to its WebDAV root."""
    return "/".join(urllib.parse.quote(p, safe="") for p in path.split("/") if p)


REFUSED = "Nextcloud refused the app password; it is not tried again until a new one is saved in the config panel."
# Nextcloud counts failed logins per address, and the units all come from 127.0.0.1.
THROTTLED = ("Nextcloud is holding back logins from this server after failed ones; "
             "as root, occ security:bruteforce:reset 127.0.0.1 clears it.")


class Nextcloud:
    """One Nextcloud account over WebDAV. It sends PROPFIND and GET, and PUT
    and MKCOL for Folders.write only: never DELETE, MOVE or COPY.

    After one refusal it sends nothing more: each failed login counts against
    every app on loopback. refused carries that across runs through mark, a
    short hash of the password, kept in the status file by the indexer.

    url is the account's WebDAV root as reached, such as
    https://127.0.0.1/remote.php/dav/files/<user>; host, when set, is the name
    Nextcloud answers to, sent as Host and checked on its certificate.
    """

    def __init__(self, url, host, user, password):
        self.base = urllib.parse.unquote(urllib.parse.urlsplit(url).path).rstrip("/")
        self.client = httpx2.Client(base_url=url.rstrip("/") + "/", auth=(user, password), timeout=60,
                                    headers={"Host": host} if host else {})
        self.extensions = {"sni_hostname": host} if host else {}
        self.mark = hashlib.sha256(password.encode()).hexdigest()[:16]
        self.refused = False

    def answer(self, resp):
        """resp, or NextcloudRefused or NextcloudError for a refused password or a throttled server."""
        if resp.status_code == 401:
            self.refused = True
            raise NextcloudRefused(REFUSED)
        if resp.status_code == 429:
            raise NextcloudError(THROTTLED)
        return resp

    @classmethod
    def from_env(cls, environ):
        """The account in DOCS_NEXTCLOUD_URL, _HOST, _USER and _PASSWORD, or None."""
        if not environ.get("DOCS_NEXTCLOUD_URL"):
            return None
        return cls(environ["DOCS_NEXTCLOUD_URL"], environ.get("DOCS_NEXTCLOUD_HOST", ""),
                   environ["DOCS_NEXTCLOUD_USER"], environ["DOCS_NEXTCLOUD_PASSWORD"])

    def list(self, path, depth):
        """[Entry] of path then, at depth 1, what it holds; None when it does not exist."""
        if self.refused:
            raise NextcloudRefused(REFUSED)
        try:
            resp = self.client.request("PROPFIND", quote(path), content=PROPFIND, extensions=self.extensions,
                                       headers={"Depth": str(depth), "Content-Type": "application/xml"})
        except httpx2.HTTPError as e:
            raise NextcloudError(f"Nextcloud did not answer: {e}") from None
        if self.answer(resp).status_code == 404:
            return None
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
        if self.refused:
            raise NextcloudRefused(REFUSED)
        data = bytearray()
        try:
            with self.client.stream("GET", quote(path), extensions=self.extensions) as resp:
                if self.answer(resp).status_code == 404:
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

    def send(self, method, path, data, expected):
        """The status of a PUT or MKCOL, one of expected; a refusal is a ValueError."""
        if self.refused:
            raise NextcloudRefused(REFUSED)
        try:
            resp = self.client.request(method, quote(path), content=data, extensions=self.extensions)
        except httpx2.HTTPError as e:
            raise NextcloudError(f"Nextcloud did not answer: {e}") from None
        if self.answer(resp).status_code == 403:
            raise ValueError("Nextcloud does not let the app's account change it.")
        if resp.status_code not in expected:
            raise NextcloudError(f"Nextcloud answered {resp.status_code}.")
        return resp.status_code


def group(fd, parent):
    """Gives what fd opens its parent folder's group, unless a setgid folder did already."""
    gid = os.fstat(parent).st_gid
    if os.fstat(fd).st_gid != gid:
        os.fchown(fd, -1, gid)


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

    def write(self, label, parts, data):
        """Write data to the file at label/parts, creating it and its missing
        folders, or replacing it; returns "created" or "replaced". With data
        None, make the folder at label/parts and those above it; returns
        "created" or "exists". Never through a link, never in place of a
        folder. The caller checks that the
        folder is writable and that parts holds no "..", no hidden name.

        On disk, what it creates takes its folder's group, and with the
        unit's UMask=0007 mode 660 or 770, so the folder's owner can change
        it; a replaced file keeps its owner and mode. Refused when the disk
        would be left with less than MIN_FREE_BYTES.
        """
        root, shown = self.roots[label], "/".join([label, *parts])
        if not kept(self.rules, shown, data is None):
            raise ValueError("the folder's filters leave this name out, so it could not be read back.")
        if root.startswith(NEXTCLOUD):
            where = [p for p in root[len(NEXTCLOUD):].split("/") if p] + parts
            base = len(where) - len(parts)
            made = False
            for n in range(base + 1, len(where) + (data is None)):
                entries = self.nextcloud.list("/".join(where[:n]), 0)
                made = entries is None
                if made:
                    self.nextcloud.send("MKCOL", "/".join(where[:n]), None, (201,))
                elif not entries[0].is_dir:
                    raise ValueError(f"{'/'.join([label, *parts[:n - base]])} is a file.")
            if data is None:
                return "created" if made else "exists"
            here = self.nextcloud.list("/".join(where), 0)
            if here and here[0].is_dir:
                raise ValueError("is a folder.")
            return "created" if self.nextcloud.send("PUT", "/".join(where), data, (201, 204)) == 201 else "replaced"
        # Each folder is opened from the one above it, never by its path, so
        # no link swapped in on the way is followed.
        d = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            made = False
            for n, name in enumerate(parts if data is None else parts[:-1], 1):
                try:
                    os.mkdir(name, 0o770, dir_fd=d)
                    made = True
                except FileExistsError:
                    made = False
                try:
                    inner = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=d)
                except OSError as e:
                    if e.errno in (errno.ENOTDIR, errno.ELOOP):
                        raise ValueError(f"{'/'.join([label, *parts[:n]])} is a file or a link.") from None
                    raise
                if made:
                    try:
                        group(inner, d)
                    except OSError:
                        os.close(inner)
                        os.rmdir(name, dir_fd=d)
                        raise
                os.close(d)
                d = inner
            if data is None:
                return "created" if made else "exists"
            free = os.fstatvfs(d)
            if free.f_bavail * free.f_frsize < len(data) + MIN_FREE_BYTES:
                raise ValueError("the disk is nearly full; nothing was written.")
            # Never through a link; O_NONBLOCK so a pipe cannot hang the open.
            flags = os.O_WRONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
            try:
                fd = os.open(parts[-1], flags | os.O_CREAT | os.O_EXCL, 0o660, dir_fd=d)
                try:
                    group(fd, d)
                except OSError:
                    os.close(fd)
                    os.unlink(parts[-1], dir_fd=d)
                    raise
                outcome = "created"
            except FileExistsError:
                try:
                    fd = os.open(parts[-1], flags, dir_fd=d)
                except OSError as e:
                    if e.errno == errno.ELOOP:
                        raise ValueError("is a link; it is not replaced.") from None
                    raise
                outcome = "replaced"
        finally:
            os.close(d)
        with os.fdopen(fd, "wb") as f:
            st = os.fstat(fd)
            if not stat.S_ISREG(st.st_mode):
                raise ValueError("is not a plain file.")
            # A hard link could be a file outside the folder.
            if st.st_nlink > 1:
                raise ValueError("has other names (hard links); it is not replaced.")
            f.truncate()
            f.write(data)
        return outcome

    def open(self, item, limit):
        """A binary file to read the Item from. A Nextcloud file is fetched whole, up to limit."""
        if item.nextcloud:
            return io.BytesIO(self.nextcloud.get(item.where, limit))
        return open(item.where, "rb")
