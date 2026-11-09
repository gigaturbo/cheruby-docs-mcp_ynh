# BACKLOG

A YunoHost app on the homelab that lets each user's AI agents, through the
Whoopy gateway, list, find, read and then search by meaning that user's own
folders, without writing anything and without waking the desktop.

## Categories

| Letter | Covers |
|---|---|
| `M` | The MCP tools and what they can reach |
| `P` | The packaging, the lifecycle scripts and the sandbox |

## Bugs

Nothing open.

## Features

### F-M-5 · search homelab documents by meaning

`todo` `large` `filed 2026-11-07`

Agents can find a document only by its name. A question such as "the letter
about the rent increase" needs a search by meaning, with the index and the
embedding model on the Pi, no wake: 16 GB of RAM, about 10 GB available on
2026-11-07, and a 300M model takes about 0.5 GB.

**Do:** `llama-server --embedding` on loopback, its own service, with
EmbeddingGemma 300M as a Q8 GGUF (Qwen3-Embedding 0.6B if results disappoint),
also registered in Whoopy as an embedding model. A background timer indexes
the text layer (248 of the 298 PDFs have one) and marks scans and images as
waiting for F-M-6; passages of 300 to 500 tokens with overlap, never across a
PDF page; text, vector, path, page, date and file fingerprint in one SQLite
file per user, with full-text search, the vectors stored as arrays and scored
with numpy (about 3,000 passages: no need for sqlite-vec); a rescan redoes
changed files and drops deleted ones. `search_documents(query, filters)`
merges meaning and keyword search and returns 5 to 10 passages with path and
page.

### F-M-6 · read scans and images into the search index

`todo` `medium` `filed 2026-11-09`

After F-M-5, the 50 PDFs without a text layer and the images are listed as
waiting but cannot be found by meaning.

**Do:** a nightly timer sends the waiting pages to qwen3-vl on the desktop
through Whoopy on loopback, with a key of its own allowed that model only;
one wake a night, none when nothing waits. Pages rendered with `pypdfium2`
from the locked wheels. The read text is kept by file fingerprint, so a new
embedding model does not redo it.

## Tests

Nothing open.

## Closed

- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
- `T-P-2` done `playtest` 2026-11-09 at `ee3c119` · backup, remove, restore: the same token came back and answers; no log holds it
- `T-P-1` done `playtest` 2026-11-09 at `ee3c119` · 401 without the token; a file that was mode 600 reads; config.php, Syncthing's config, PHP sessions, the rest of Nextcloud and Syncthing, the author's home and the photo disk are absent in the unit; the folders are read-only; no capabilities; the token is in no install log
- `T-M-1` done `spec` 2026-11-07 at `3e6f8cc` · `tests/check_server.py`: 21 of 21 pass on the locked wheels
