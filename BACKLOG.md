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

- `F-M-5` done `large` 2026-11-09 · `search_documents` by meaning and keywords on an hourly index (llama-server built on the Pi, EmbeddingGemma 300M, numpy, FTS5): 288 files indexed in 31 min, 10 of 10 test documents in the top 5; healthchecks.io pinged; the model offered through Whoopy as `embeddinggemma` (Whoopy F-P-2)
- `T-P-2` done `playtest` 2026-11-09 at `315b90c` · backup, remove, restore: same token (by fingerprint), answers, ping settings back, index cleared and rebuilt by the timer; the shared operation logs hold no token, but yunohost-cli.log (root-only) has held it since the first install, from the `secret_token=` trace line
- `T-M-2` done `playtest` 2026-11-09 at `bb9fd44` · 10 of 10 real documents found in the top 5 by queries in other words, French and English, 7 at rank 1; 0.12 to 0.18 s a search
- `T-P-1` done `playtest` 2026-11-09 at `bb9fd44` · 401 without the token; tool server and indexer see the folders, the index and their code only (Nextcloud data only as the empty path to Notes); the embedding unit sees its binary and model only; the server cannot write the index, the indexer cannot write its code or the folders; `/slots` gives 501; no capabilities
- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
- `T-M-1` done `spec` 2026-11-09 · `tests/check_server.py`: 45 of 45 pass on the locked wheels, search and indexer included
