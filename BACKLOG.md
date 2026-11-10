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

### F-M-7 · a token for Whoopy, for a chosen user and folders, shown in the panel

`wip` `medium` `filed 2026-11-10`

From its ~ynh9, Whoopy no longer reads this app's settings. It serves one
`docs` MCP server at `/mcp/docs`, with an address and a token pasted in its
own panel (Whoopy F-M-8). Until this item, the token pasted there is a user's
own `token_<user>`, read by `yunohost app setting cheruby-docs-mcp
token_<user>`.

**Do:** a "Whoopy access" section in the panel: a user (install question,
type `user`, default the first admin), the folders it opens (a subset of that
user's, by label), the token shown so it can be copied, and a "New token"
button that makes another one and restarts the service. That token reuses the
user's index, so nothing is indexed twice. The token's trace must not reach a
log, `yunohost-cli.log` included (see T-P-2).

Built, waiting for T-P-3: the panel has a "Whoopy access" section (user,
folders) and a "Whoopy token" one (the token, its address, "New token"); a
section holding a button saves nothing, hence two. The per-user tokens stay,
and no token is traced any more, at configure either.

## Tests

### T-P-3 · the Whoopy token, from the panel, stays out of the logs

`todo` `playtest` `filed 2026-11-10`

Recipe in the `cheruby-docs-mcp-kb` skill. Covers F-M-7 on the server: the
token shows in the panel, narrowing the folders narrows list and search, "New
token" retires the old one, and no new line of `yunohost-cli.log` or of the
config operation logs holds the token.

## Closed

- `F-M-5` done `large` 2026-11-09 · `search_documents` by meaning and keywords on an hourly index (llama-server built on the Pi, EmbeddingGemma 300M, numpy, FTS5): 288 files indexed in 31 min, 10 of 10 test documents in the top 5; healthchecks.io pinged; the model offered through Whoopy as `embeddinggemma` (Whoopy F-P-2)
- `T-P-2` done `playtest` 2026-11-09 at `315b90c` · backup, remove, restore: same token (by fingerprint), answers, ping settings back, index cleared and rebuilt by the timer; the shared operation logs hold no token, but yunohost-cli.log (root-only) has held it since the first install, from the `secret_token=` trace line
- `T-M-2` done `playtest` 2026-11-09 at `bb9fd44` · 10 of 10 real documents found in the top 5 by queries in other words, French and English, 7 at rank 1; 0.12 to 0.18 s a search
- `T-P-1` done `playtest` 2026-11-09 at `bb9fd44` · 401 without the token; tool server and indexer see the folders, the index and their code only (Nextcloud data only as the empty path to Notes); the embedding unit sees its binary and model only; the server cannot write the index, the indexer cannot write its code or the folders; `/slots` gives 501; no capabilities
- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
- `T-M-1` done `spec` 2026-11-09 · `tests/check_server.py`: 45 of 45 pass on the locked wheels, search and indexer included
