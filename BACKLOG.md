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
passes its offline check; the app is not installed yet.

**Do:** install it with Syncthing's `Documents` and Nextcloud's `Notes` for
the author, pass T-P-1, then register `docs_<user>` in Whoopy (its F-M-6).

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

### T-P-1 · the installed server reads only its folders

`todo` `playtest` `filed 2026-11-09` `needs: F-M-4`

**Do:** install with the author's two folders, then call the server on
loopback with and without the token, read a file that was mode 600 before
the 2026-11-07 chmod, and try `/var/www/nextcloud/config/config.php` and a
PHP session from inside the unit. **Pass:** 401 without the token; the file
reads; the rest is not there for the process.

### T-P-2 · backup and restore keep the tokens

`todo` `playtest` `filed 2026-11-09` `needs: T-P-1`

**Do:** `yunohost backup create --apps cheruby-docs-mcp`, remove, restore.
**Pass:** the token from before the backup still works.

## Closed

- `T-M-1` done `spec` 2026-11-07 at `3e6f8cc` · `tests/check_server.py`: 21 of 21 pass on the locked wheels
