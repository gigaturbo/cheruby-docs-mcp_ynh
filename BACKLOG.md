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

`wip` `large` `filed 2026-11-07`

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

**State 2026-11-09:** written and checked offline (T-M-1); llama-server
b12223 builds on the Pi in 8 min and EmbeddingGemma answered with the unit's
flags: 683 MB, 1.1 s a passage, 0.04 s a query. Left: install 0.2.0 (ask
first), T-P-1, T-P-2, T-M-2. Whoopy registration after that, asked
separately.

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

### T-M-2 · search finds documents by meaning on the real folders

`todo` `playtest` `filed 2026-11-09`

Once the first index run is done (`journalctl -u cheruby-docs-mcp-index`),
ask search_documents for five documents by what they are about, in other
words than theirs, French and English: "the letter about the rent increase",
"mon dernier bulletin de salaire", ... Pass: each is in the top 5. If not,
try Qwen3-Embedding 0.6B.

### T-P-1 · the installed server reads only its folders

`todo` `playtest` `reopened 2026-11-09`

Reopened by F-M-5: two new units, the index folder bound into the server.
Also check that the embedding unit sees only its binary and model, that
`curl 127.0.0.1:8771/slots` is refused, and that the indexer writes only in
the index folder. Last result: done 2026-11-09 at `ee3c119`.

### T-P-2 · backup and restore keep the tokens

`todo` `playtest` `reopened 2026-11-09`

Reopened by F-M-5: restore now builds llama-server and fetches the model,
and clears the index for the timer to rebuild. Last result: done 2026-11-09
at `ee3c119`.

## Closed

- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
- `T-M-1` done `spec` 2026-11-09 · `tests/check_server.py`: 45 of 45 pass on the locked wheels, search and indexer included
