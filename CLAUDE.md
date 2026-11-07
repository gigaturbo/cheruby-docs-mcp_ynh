# CLAUDE.md

A YunoHost package for an MCP tool server that lets AI agents list, find and
read each user's homelab folders, read-only, behind the Whoopy AI gateway.
Search by meaning comes later (backlog F-M-5 in whoopy_ynh).

The host's own `CLAUDE.md` still applies, and its rules win: installing or
upgrading this app touches a running service, so ask first.

## Layout

- `conf/cheruby_docs_mcp.py`: the tool server, the whole of the code.
- `conf/requirements.in` → `conf/requirements.txt`: the hash-locked wheels.
  The re-lock command is at the top of `requirements.in`.
- `tests/check_server.py`: the offline check, on throwaway folders.
- `manifest.toml`, `scripts/`: packaging format 2, helpers 2.1 (to come).

## Architecture

- **One service**, loopback only. Whoopy reaches it and holds one entry per
  user (`docs_<user>`), each with that user's token.
- **The token picks the user.** A wrapper in front of the MCP app matches the
  bearer token and puts that user's folders in the request state; the tools
  read them from there. Tool paths start with a folder label, never a real path.
- **Read access through groups, not ACLs.** The unit joins the `syncthing` and
  `nextcloud` groups for itself, and the sandbox shows it only the configured
  folders, read-only. Syncthing's `Documents` is set to Ignore Permissions on
  the server, so synced files keep group read.

## Commands

```bash
uv venv .venv --python /usr/bin/python3
uv pip sync --python .venv/bin/python conf/requirements.txt && uv pip install --python .venv/bin/python httpx
.venv/bin/python tests/check_server.py
```
