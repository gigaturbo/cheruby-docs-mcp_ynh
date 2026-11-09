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

### F-M-4 · list and read homelab files through MCP

`wip` `large` `filed 2026-11-06`

Agents cannot see the files kept on the Pi. The tool server is written and
is installed on the server with the author's two folders and passes T-P-1;
no agent reaches it until Whoopy registers it.

**Do:** register `docs_<user>` in Whoopy (its F-M-6), then have an agent list,
find and read through it.

### F-M-5 · search homelab documents by meaning

`todo` `large` `filed 2026-11-07` `needs: F-M-4`

Agents can find a document only by its name. A question such as "the letter
about the rent increase" needs a search by meaning, with the index and the
embedding model on the Pi, no wake: 16 GB of RAM, about 10 GB available on
2026-11-07, and a 300M model takes about 0.5 GB.

**Do:** `llama-server --embedding` on loopback, its own service, with
EmbeddingGemma 300M as a Q8 GGUF (Qwen3-Embedding 0.6B if results disappoint),
also registered in Whoopy as an embedding model. A background timer indexes:
text layer first (248 of the 298 PDFs have one), scans and images queued for
OCR by qwen3-vl on the desktop through Whoopy, one wake a night when the
queue is not empty; passages of 300 to 500 tokens with overlap; text, vector,
path, page, date and file fingerprint in one SQLite file per user, with
sqlite-vec and full-text search; a rescan redoes changed files and drops
deleted ones. `search_documents(query, filters)` merges meaning and keyword
search and returns 5 to 10 passages with path and page.

## Tests

Nothing open.

## Closed

- `T-P-2` done `playtest` 2026-11-09 at `ee3c119` · backup, remove, restore: the same token came back and answers; no log holds it
- `T-P-1` done `playtest` 2026-11-09 at `ee3c119` · 401 without the token; a file that was mode 600 reads; config.php, Syncthing's config, PHP sessions, the rest of Nextcloud and Syncthing, the author's home and the photo disk are absent in the unit; the folders are read-only; no capabilities; the token is in no install log
- `T-M-1` done `spec` 2026-11-07 at `3e6f8cc` · `tests/check_server.py`: 21 of 21 pass on the locked wheels
