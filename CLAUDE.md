# CLAUDE.md

A YunoHost package for an MCP tool server that lets each user's AI agents
list, find, read and search by meaning that user's homelab folders,
read-only, behind the Whoopy AI gateway.

The host's own `CLAUDE.md` still applies, and its rules win: installing or
upgrading this app touches a running service, so ask first. Durable
constraints and the check recipes are in the `cheruby-docs-mcp-kb` skill.

## Layout

- `conf/cheruby_docs_mcp.py`: the tool server.
- `conf/docs_index.py`: the indexer, and the index format the server reads.
- `conf/docs_files.py`: what both know about the folders and their files.
- `conf/systemd.service`, `embed.service`, `index.service`, `index.timer`:
  the sandboxed units.
- `conf/requirements.in` → `conf/requirements.txt`: the hash-locked wheels.
  The re-lock command is at the top of `requirements.in`.
- `manifest.toml`, `scripts/`: packaging format 2, helpers 2.1. Shared steps
  in `scripts/_common.sh`.
- `tests/check_server.py`: the offline check, on throwaway folders, with a
  stand-in embedding server.

## Architecture

- **Three units, one user** `cheruby-docs-mcp`, all on loopback:
  - `cheruby-docs-mcp.service`, the tool server (uvicorn). Whoopy reaches it
    and holds one entry per user (`docs_<user>`).
  - `cheruby-docs-mcp-embed.service`, llama-server with EmbeddingGemma 300M,
    built from source at install.
  - `cheruby-docs-mcp-index.service`, run hourly by its timer: walks the
    folders and updates each user's search index.
- **Code and secrets** in `/var/www/cheruby-docs-mcp`, root-owned, group
  read-only: the venv, the Python modules, `bin/llama-server`, `model/`,
  `docs.env` (root 600, tokens and folders) and `index.env` (root 600, folders
  only).
- **The index is the only state**, one SQLite file per user in
  `/home/yunohost.app/cheruby-docs-mcp`, rebuilt from the folders, never
  backed up. The `folders` and `token_<user>` settings are the rest;
  configure renders the env files and the units from them.
- **The token picks the user**, and the unit joins each folder's group to
  read it, with everything else those groups could read hidden.

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
