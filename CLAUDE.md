# CLAUDE.md

A YunoHost package for an MCP tool server that lets each user's AI agents
list, find and read that user's homelab folders, read-only, behind the Whoopy
AI gateway.

The host's own `CLAUDE.md` still applies, and its rules win: installing or
upgrading this app touches a running service, so ask first. Durable
constraints and the check recipes are in the `cheruby-docs-mcp-kb` skill.

## Layout

- `conf/cheruby_docs_mcp.py`: the tool server, the whole of the code.
- `conf/systemd.service`: the sandboxed unit.
- `conf/requirements.in` → `conf/requirements.txt`: the hash-locked wheels.
  The re-lock command is at the top of `requirements.in`.
- `manifest.toml`, `scripts/`: packaging format 2, helpers 2.1. Shared steps
  in `scripts/_common.sh`.
- `tests/check_server.py`: the offline check, on throwaway folders.

## Architecture

- **One service**, `cheruby-docs-mcp.service`, one uvicorn process on
  loopback, user `cheruby-docs-mcp`. Whoopy reaches it and holds one entry per
  user (`docs_<user>`).
- **Code and secrets** in `/var/www/cheruby-docs-mcp`, root-owned, group
  read-only: the venv, the server, and `docs.env` (root 600) with each user's
  token and folders.
- **No state.** The `folders` setting and the `token_<user>` settings are all
  there is; configure renders `docs.env` and the unit from them.
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
Restore rebuilds the code and needs the network.
