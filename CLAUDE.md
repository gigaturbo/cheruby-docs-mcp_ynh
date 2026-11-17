# CLAUDE.md

A YunoHost package for an MCP tool server that lets one user's AI agents
list, find, read and search by meaning that user's homelab folders, and
write text files in those marked writable, behind the Whoopy AI gateway.
The folders come from sources (Syncthing, local storage, Nextcloud) chosen
in the config panel.

The host's own `CLAUDE.md` still applies, and its rules win: installing or
upgrading this app touches a running service, so ask first. Durable
constraints and the check recipes are in the `cheruby-docs-mcp-kb` skill.

## Layout

- `conf/cheruby_docs_mcp.py`: the tool server.
- `conf/docs_index.py`: the indexer, and the index format the server reads.
- `conf/docs_files.py`: what both know about the folders, their filters
  and their files.
- `conf/docs_text.py`: the text inside a document, PDF, office, EPUB or
  email, for both.
- `conf/systemd.service`, `embed.service`, `rerank.service`, `index.service`, `index.timer`:
  the sandboxed units.
- `conf/requirements.in` → `conf/requirements.txt`: the hash-locked wheels.
  The re-lock command is at the top of `requirements.in`.
- `config_panel.toml`, `scripts/config`: the sources, the index schedule
  and its runs, health pings and the Whoopy token.
- `manifest.toml`, `scripts/`: packaging format 2, helpers 2.1. Shared steps
  in `scripts/_common.sh`.
- `tests/eval_search.py`: scores search on a running server against
  a private set of queries (T-M-2).
- `tests/check_server.py`: the offline check, on throwaway folders, with a
  stand-in embedding server and a stand-in Nextcloud.

## Architecture

- **Four units, one user** `cheruby-docs-mcp`, all on loopback:
  - `cheruby-docs-mcp.service`, the tool server (uvicorn). Whoopy reaches it
    with one token, chosen in the config panel: one user, some of their folders.
  - `cheruby-docs-mcp-embed.service`, llama-server built from source at
    install, with the embedding model chosen in the panel: EmbeddingGemma
    300M (default), jina-embeddings-v5-text-nano or Qwen3-Embedding 0.6B,
    each Q8 or Q4.
  - `cheruby-docs-mcp-rerank.service`, the same llama-server with
    jina-reranker-v2 (non-commercial licence): it orders search results.
  - `cheruby-docs-mcp-index.service`, run by its timer on the schedule
    chosen in the panel, or by "Run now": walks the folders, updates the
    user's search index and writes `<user>.status.json` for the panel.
- **Code and secrets** in `/var/www/cheruby-docs-mcp`, root-owned, group
  read-only: the venv, the Python modules, `bin/llama-server`, `model/`,
  `folders.json` (the folders and their filters), `docs.env` (root 600, the
  tokens) and `index.env` (root 600, no MCP token). Both hold the Nextcloud app
  password when a folder is in Nextcloud.
- **The index is the only state**, one SQLite file for the user in
  `/home/yunohost.app/cheruby-docs-mcp`, rebuilt from the folders, never
  backed up. The settings are the rest: `docs_user`, each source's
  `<source>_enabled`, `_folders`, `_only` and `_skip`, `token_<user>`,
  `<source>_writable`, `nextcloud_account`, `nextcloud_password`,
  `index_schedule`, `index_calendar`, `index_quiet`, `embed_model` and `whoopy_*`; configure renders `folders.json`, the
  env files and the units from them.
- **The token picks the user**, and the unit joins each folder's group to
  read it, and to change it when it is writable and mounted read-write,
  with everything else those groups could read hidden. Nextcloud's folders
  are read and written through Nextcloud over loopback, never on disk.

## Commands

```bash
uv venv .venv --python /usr/bin/python3
uv pip sync --python .venv/bin/python conf/requirements.txt && uv pip install --python .venv/bin/python httpx
.venv/bin/python tests/check_server.py
for f in scripts/*; do bash -n "$f"; done
```

## Packaging

The whole repository ships except `CLAUDE.md`, `BACKLOG.md` and `TODO.md`.
Backup holds the unit; the folders and tokens travel in the app settings.
Restore rebuilds the code and llama-server (about 8 minutes) and needs the
network; the timer rebuilds the index.
