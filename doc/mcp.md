# MCP tools

## Connecting

The server listens on `http://127.0.0.1:<port>/mcp` (Streamable HTTP), on
the server itself only. Every request needs `Authorization: Bearer <token>`;
without it the answer is 401.

One token opens it, on all the user's folders or the ones listed under
"MCP access" in the config panel. Search keeps to them too. See the
address and the token, and make a new token:

```bash
sudo cheruby-docs-mcp token
sudo cheruby-docs-mcp token --new
```

`--new` presses the panel's "New token" button: the old token stops working
at once, so give the new one to every client.

A client on the server connects straight to the address. One elsewhere goes
through a gateway on the server that publishes the tools, such as Whoopy
(the address and the token as an MCP server, on its status page, MCP
Servers tab) or LiteLLM (an MCP server with that URL, HTTP transport and the
token as Bearer).

Paths start with a folder's label, such as `Documents/Factures`.

## What the tools cannot do

- **No delete.** `write_file` creates or replaces a file whole,
  `create_folder` creates folders and `move_file` moves or renames them;
  nothing removes a file or a folder, and a move never replaces one.
- **Write only in the folders marked writable**, text files only, never
  through a link, and never into the last 2 GB of the disk. The three write
  tools are offered only when a folder is writable.
- **A file open in Nextcloud's Text editor cannot be changed.** Text locks
  the file while it is open, and Nextcloud refuses every other write. Up
  to Text 8.0.1 (Nextcloud 34) and 9.0.2 (Nextcloud 35) the lock can outlive
  the editor, for days: an upstream bug (nextcloud/text#9042), fixed in
  Nextcloud 34.0.7 and 35.0.4. Until then the admin
  unlocks it with `occ files:lock --unlock <file id> <user>`, once nobody
  has it open.
- **Only the token's folders.** Search keeps to them too.
- **No scans or images in search yet.** Find them by name with `find_files`;
  `read_file` returns an image as the picture.
- **Bounded reads.** 20 pages of a PDF or 20,000 characters at a time.
- **Bounded transcriptions.** One recording at a time, 5 at most running
  or waiting, the first 4 hours of each, up to 1 GB on disk and 64 MB in
  Nextcloud. A transcript is kept 30 days after it was last read.
- **Bounded index runs.** `run_index` takes 50 files at most, and is refused
  in the index's quiet hours, with 20 requests waiting, and for all the
  folders within 15 minutes of the last full run.

## list_folder

A folder's subfolders, then its files with size and date.

| Argument | Default | Effect |
|---|---|---|
| `path` | empty | The folder, such as `Documents/Factures`. Empty lists the top folders. |

## find_files

Files and folders whose path holds every word of the query, ignoring case
and accents. Names only, not contents.

| Argument | Default | Effect |
|---|---|---|
| `query` | required | The words to find in the path. |
| `path` | empty | Limits the search to one folder, such as `Documents/Santé`. |

## read_file

One file: text and Markdown by character, PDF by page, images as pictures.
Word, Excel and PowerPoint (docx, xlsx, pptx), OpenDocument (odt, ods, odp),
EPUB books and saved emails (eml) come back as text, with their headings as
Markdown `#` lines.

| Argument | Default | Effect |
|---|---|---|
| `path` | required | The file. |
| `page` | 1 | A PDF's first page to read. |
| `pages` | 5 | How many PDF pages, 1 to 20. |
| `offset` | 0 | Other text: the character to start at. |
| `max_chars` | 8000 | Other text: how many characters, up to 20,000. |

## search_documents

The best passage of each matching file, found by meaning and by keywords,
then ordered by how well it answers the query. Each passage starts with the
file's headings above it, and comes with its page and date. Searches an index
refreshed on the schedule chosen in the panel.

| Argument | Default | Effect |
|---|---|---|
| `query` | required | What the passage says, in the user's words. |
| `path` | empty | Limits the search to one folder, such as `Documents/Logement`. |
| `after` | empty | Files dated on or after this day, as `2025-03-31`. |
| `before` | empty | Files dated on or before this day, as `2025-03-31`. |
| `limit` | 8 | How many files, 1 to 10. |

## write_file

Creates a text file, with its missing folders, or replaces one whole, in a
writable folder.

| Argument | Default | Effect |
|---|---|---|
| `path` | required | Such as `Notes/Recettes/pain.md`. Text files only (`.md`, `.txt`, `.csv`...). |
| `content` | required | The whole new text of the file, UTF-8. |

## create_folder

Creates a folder, with those above it, in a writable folder. A folder that
exists already is left as it is.

| Argument | Default | Effect |
|---|---|---|
| `path` | required | Such as `Notes/Recettes/Pains`. |

## move_file

Moves or renames a file or a folder, of any kind, within one writable
folder. Never replaces what is at the new place, never moves a link, never
a folder into itself. A folder whose files the folder's filters would judge
otherwise at its new place is not moved; move its files instead. In
Nextcloud, nothing moves into or out of a folder shared from another
account. Search finds it under its new name after the next index run.

| Argument | Default | Effect |
|---|---|---|
| `source` | required | Such as `Notes/brouillon.md`. |
| `destination` | required | Its new path in the same folder, such as `Notes/Recettes/pain.md`. The folder it goes in must exist (`create_folder`). |

## run_index

Asks for an index run, so `search_documents` finds what changed: of some
files, such as one just written, or of all the folders, which redoes only
what changed. With files, it waits up to a minute and says how each went.
Changes no file.

| Argument | Default | Effect |
|---|---|---|
| `files` | empty | Up to 50 paths, such as `Notes/Recettes/pain.md`. Empty asks for all the folders. |

## transcribe_file

The text of a recording: mp3, m4a, aac, wav, ogg, opus, flac, webm, mka,
amr, 3gp or mp4 (its sound only). It is cut in parts of about ten minutes,
each ending at a pause, and transcribed by whisper on the server or, when
the config panel allows it, by the desktop's GPU through the
OpenAI-compatible server set there. The
language is detected on the first part and kept for the rest.

An hour of audio takes about 15 minutes on the server with whisper small,
about an hour with large-v3-turbo.
The tool waits up to a minute, then says how far it is: call it again with
the same path later. What is done is kept, through a restart too. Once
done, the text comes back at once, read by character like `read_file`. A
recording changed since, or asked for in another language, is transcribed
again; one changed before its turn comes is refused, to be asked again. It changes no file: save the cleaned text with `write_file`, then
move the recording with `move_file`.

| Argument | Default | Effect |
|---|---|---|
| `path` | | Such as `Notes/Dictaphone/réunion.m4a`. |
| `engine` | the panel's | `pi` for this server, `desktop` for the desktop's GPU when set up. A desktop that does not answer within three minutes leaves the rest to the server. |
| `language` | detected | Such as `fr` or `french`. |
| `timestamps` | true | Each line starts with its time, `[1:02:05]`; false gives paragraphs split at pauses. |
| `retry` | false | Try again a recording that failed. Without it, the failure is said again, with where it happened. |
| `offset`, `max_chars` | 0, 8000 | Where to start reading the text and how much, up to 20,000 characters. |

The first line says how long the recording is, its language and where it
was transcribed.
