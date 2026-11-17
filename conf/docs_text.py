"""The text inside a document, for the tool server and the indexer.

document_text reads Word, OpenDocument, PowerPoint, Excel, EPUB and email
files from a binary file: one paragraph, row or header a line, headings as
Markdown "#" lines, which the indexer cuts passages at. Each format is read
from its XML with the standard library. open_pdf opens a PDF. Errors a user
should see are ValueError.
"""

import email
import email.policy
import logging
import posixpath
import re
import urllib.parse
import zipfile
from html.parser import HTMLParser
from xml.etree import ElementTree

from pypdf import PdfReader

# pypdf warns on every odd font; the journal needs its errors only.
logging.getLogger("pypdf").setLevel(logging.ERROR)

# What a document may unpack to in all, so a small file cannot fill the memory.
MAX_XML_BYTES = 20_000_000

WORD = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
SHEET = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
SLIDE = "{http://schemas.openxmlformats.org/presentationml/2006/main}"
DRAWING = "{http://schemas.openxmlformats.org/drawingml/2006/main}"
RELATION = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
ODF_TEXT = "{urn:oasis:names:tc:opendocument:xmlns:text:1.0}"
ODF_TABLE = "{urn:oasis:names:tc:opendocument:xmlns:table:1.0}"
ODF_DRAW = "{urn:oasis:names:tc:opendocument:xmlns:drawing:1.0}"
ODF_SHOW = "{urn:oasis:names:tc:opendocument:xmlns:presentation:1.0}"
EPUB_CONTAINER = "{urn:oasis:names:tc:opendocument:xmlns:container}"
OPF = "{http://www.idpf.org/2007/opf}"
# Word names its heading styles in the document's language: Heading1, Titre1.
HEADING_STYLE = re.compile(r"(?i)^(?:heading|titre)\s*([1-6])$")


class Package:
    """The members of a zipped document, read up to MAX_XML_BYTES in all."""

    def __init__(self, z):
        self.z, self.left = z, MAX_XML_BYTES

    def read(self, name):
        try:
            size = self.z.getinfo(name).file_size
        except KeyError:
            raise ValueError(f"the document has no {name or 'main part'}; it may be damaged.") from None
        if size > self.left:
            raise ValueError("the document's text is too large to read.")
        self.left -= size
        return self.z.read(name)

    def xml(self, name):
        return ElementTree.fromstring(self.read(name))

    def parts(self, part):
        """{relationship id: member} of an Office Open XML part."""
        folder, name = posixpath.split(part)
        return {r.get("Id"): member(folder, r.get("Target", ""))
                for r in self.xml(posixpath.join(folder, "_rels", name + ".rels"))}


def member(folder, href):
    """The zip member a link from a part in folder names."""
    path = urllib.parse.unquote(href.split("#")[0])
    return posixpath.normpath(path[1:] if path.startswith("/") else posixpath.join(folder, path))


def heading_level(el):
    """1 to 6 for a heading paragraph of a docx or odt, else 0."""
    if el.tag == ODF_TEXT + "h":
        level = el.get(ODF_TEXT + "outline-level", "1")
        return min(max(int(level), 1), 6) if level.isdigit() else 1
    style = el.find(f"{WORD}pPr/{WORD}pStyle")
    match = style is not None and HEADING_STYLE.match(style.get(WORD + "val", ""))
    return int(match[1]) if match else 0


def paragraphs(root):
    """The paragraphs of a docx or odt part, one a line."""
    lines = []
    for el in root.iter():
        if el.tag.rsplit("}", 1)[-1] in ("p", "h"):
            text = "".join(el.itertext()).strip()
            if text:
                level = heading_level(el)
                lines.append(f"{'#' * level} {text}" if level else text)
    return "\n".join(lines)


def slide(n, texts):
    """Lines of slide n from its (is title, text) pairs: "# Slide <n>: <title>", then the rest."""
    texts = [(title, t.strip()) for title, t in texts if t.strip()]
    title = " ".join(t for is_title, t in texts if is_title)
    return [f"# Slide {n}: {title}" if title else f"# Slide {n}"] + [t for is_title, t in texts if not is_title]


def pptx_text(pkg):
    """A pptx's slides in the order shown, speaker notes left out."""
    deck = "ppt/presentation.xml"
    parts = pkg.parts(deck)
    lines = []
    for n, ref in enumerate(pkg.xml(deck).iter(SLIDE + "sldId"), 1):
        root = pkg.xml(parts.get(ref.get(RELATION + "id"), ""))
        titles = set()
        for shape in root.iter(SLIDE + "sp"):
            ph = shape.find(f"{SLIDE}nvSpPr/{SLIDE}nvPr/{SLIDE}ph")
            if ph is not None and ph.get("type") in ("title", "ctrTitle"):
                titles.update(shape.iter(DRAWING + "p"))
        lines += slide(n, [(p in titles, "".join(p.itertext())) for p in root.iter(DRAWING + "p")])
    return "\n".join(lines)


def odp_text(pkg):
    """An odp's slides in order, speaker notes left out."""
    lines = []
    for n, page in enumerate(pkg.xml("content.xml").iter(ODF_DRAW + "page"), 1):
        lines += slide(n, [(shape.get(ODF_SHOW + "class") == "title", "".join(p.itertext()))
                           for shape in page if shape.tag != ODF_SHOW + "notes"
                           for p in shape.iter(ODF_TEXT + "p")])
    return "\n".join(lines)


def xlsx_text(pkg):
    """An xlsx's sheets in order, each under "# <name>", a row a line, its cells
    tab-separated as stored: a date is its serial number, a formula its last result."""
    book = "xl/workbook.xml"
    parts = pkg.parts(book)
    strings = [m for m in parts.values() if m.endswith("/sharedStrings.xml")]
    shared = [] if not strings else [
        "".join(t.text or "" for t in si.findall(f"{SHEET}t") + si.findall(f"{SHEET}r/{SHEET}t"))
        for si in pkg.xml(strings[0]).iter(SHEET + "si")]
    lines = []
    for sheet in pkg.xml(book).iter(SHEET + "sheet"):
        lines.append(f"# {sheet.get('name', '')}")
        for row in pkg.xml(parts.get(sheet.get(RELATION + "id"), "")).iter(SHEET + "row"):
            cells = []
            for c in row.iter(SHEET + "c"):
                value = c.findtext(SHEET + "v") or ""
                if c.get("t") == "s":
                    value = shared[int(value)] if value.isdigit() and int(value) < len(shared) else ""
                elif c.get("t") == "inlineStr":
                    value = "".join(t.text or "" for t in c.iter(SHEET + "t"))
                cells.append(value.strip())
            if any(cells):
                lines.append("\t".join(c for c in cells if c))
    return "\n".join(lines)


def ods_text(pkg):
    """An ods's sheets in order, each under "# <name>", a row a line, its cells
    tab-separated; a row or cell repeated is given once."""
    lines = []
    for table in pkg.xml("content.xml").iter(ODF_TABLE + "table"):
        lines.append(f"# {table.get(ODF_TABLE + 'name', '')}")
        for row in table.iter(ODF_TABLE + "table-row"):
            cells = [" ".join("".join(p.itertext()).strip() for p in cell.iter(ODF_TEXT + "p")).strip() for cell in row]
            if any(cells):
                lines.append("\t".join(c for c in cells if c))
    return "\n".join(lines)


class HtmlText(HTMLParser):
    """Collects an HTML page's text, a line per block, headings as "#" lines."""
    BLOCKS = {"p", "div", "br", "li", "tr", "td", "th", "dt", "dd", "blockquote", "pre", "section", "article",
              "header", "footer", "aside", "nav", "table", "ul", "ol", "dl", "figure", "figcaption", "hr"}
    HEADINGS = {"h1", "h2", "h3", "h4", "h5", "h6"}
    SKIPPED = {"script", "style", "title"}

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts, self.skip = [], 0

    def handle_starttag(self, tag, attrs):
        if tag in self.SKIPPED:
            self.skip += 1
        elif tag in self.HEADINGS:
            self.parts.append("\n" + "#" * int(tag[1]) + " ")
        elif tag in self.BLOCKS:
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in self.SKIPPED:
            self.skip = max(0, self.skip - 1)
        elif tag in self.HEADINGS or tag in self.BLOCKS:
            self.parts.append("\n")

    def handle_data(self, data):
        if not self.skip:
            self.parts.append(data)


def html_text(page):
    """The text of an HTML page, a line per block, headings as Markdown "#" lines."""
    parser = HtmlText()
    parser.feed(page)
    parser.close()
    lines = (" ".join(line.split()) for line in "".join(parser.parts).splitlines())
    return "\n".join(line for line in lines if line.strip("# "))


def epub_text(pkg):
    """An epub's chapters in reading order."""
    rootfile = pkg.xml("META-INF/container.xml").find(f".//{EPUB_CONTAINER}rootfile")
    opf = rootfile.get("full-path", "") if rootfile is not None else ""
    book = pkg.xml(opf)
    hrefs = {item.get("id"): item.get("href", "") for item in book.iter(OPF + "item")}
    chapters = (pkg.read(member(posixpath.dirname(opf), hrefs.get(ref.get("idref"), "")))
                for ref in book.iter(OPF + "itemref"))
    return "\n".join(filter(None, (html_text(c.decode("utf-8", errors="replace")) for c in chapters)))


def eml_text(source):
    """An email: "# <subject>", its senders, recipients, date and attachments'
    names, then its text, or its HTML's when it has no plain text."""
    msg = email.message_from_bytes(source.read(MAX_XML_BYTES), policy=email.policy.default)
    lines = [f"# {msg['subject'] or '(no subject)'}"]
    lines += [f"{name}: {msg[name]}" for name in ("From", "To", "Cc", "Date") if msg[name]]
    attached = [a.get_filename() for a in msg.iter_attachments() if a.get_filename()]
    if attached:
        lines.append("Attachments: " + ", ".join(attached))
    body = msg.get_body(("plain", "html"))
    if body is not None:
        text = body.get_content()
        lines.append(html_text(text) if body.get_content_subtype() == "html" else text.strip())
    return "\n".join(lines)


ZIPPED = {
    ".docx": lambda pkg: paragraphs(pkg.xml("word/document.xml")),
    ".odt": lambda pkg: paragraphs(pkg.xml("content.xml")),
    ".pptx": pptx_text,
    ".odp": odp_text,
    ".xlsx": xlsx_text,
    ".ods": ods_text,
    ".epub": epub_text,
}
DOCUMENT_SUFFIXES = ZIPPED.keys() | {".eml"}


def document_text(source, suffix):
    """The text of a document in DOCUMENT_SUFFIXES, from a binary file."""
    if suffix == ".eml":
        return eml_text(source)
    try:
        with zipfile.ZipFile(source) as z:
            return ZIPPED[suffix](Package(z))
    except (zipfile.BadZipFile, ElementTree.ParseError) as e:
        raise ValueError(f"the document is damaged: {e}") from None


def open_pdf(source):
    """A PDF from a path or a binary file."""
    reader = PdfReader(source)
    if reader.is_encrypted and not reader.decrypt(""):
        raise ValueError("the PDF is password-protected.")
    return reader
