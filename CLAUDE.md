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

- `cheruby-docs/`: the tool server, the indexer and their commands, one
  Rust binary. `serve.rs` the MCP server, `tools.rs` the tools, `index.rs`
  the indexer and the index format, `folders.rs` and `nextcloud.rs` the
  folders and their filters, `text.rs` and `passages.rs` the text inside a
  file and how it is cut, `embed.rs` the embedding and reranking servers.
- `conf/systemd.service`, `embed.service`, `rerank.service`, `index.service`,
  `index.timer`, `index.path`: the sandboxed units.
- `config_panel.toml`, `scripts/config`: the sources, the index schedule
  and its runs, health pings and the Whoopy token.
- `manifest.toml`, `scripts/`: packaging format 2, helpers 2.1. Shared steps
  in `scripts/_common.sh`.
- `build/llama-server.sh`, `.github/workflows/llama-server.yml`: build
  llama-server for the Pi 5 and publish it as the release the manifest pins.
- `tests/eval_search.py`: scores search on a running server against
  a private set of queries (T-M-2).
- `tests/check_server.py`: the offline check, on throwaway folders, with a
  stand-in embedding server and a stand-in Nextcloud.

## Architecture

- **Four units, one user** `cheruby-docs-mcp`, all on loopback:
  - `cheruby-docs-mcp.service`, the tool server (`cheruby-docs serve`).
    Whoopy reaches it with one token, chosen in the config panel: one user,
    some of their folders.
  - `cheruby-docs-mcp-embed.service`, llama-server (a pinned release, built
    for the Pi 5 by the repository's workflow), with the embedding model chosen in the panel: EmbeddingGemma
    300M (default), jina-embeddings-v5-text-nano or Qwen3-Embedding 0.6B,
    each Q8 or Q4.
  - `cheruby-docs-mcp-rerank.service`, the same llama-server with
    jina-reranker-v2 (non-commercial licence): it orders search results.
  - `cheruby-docs-mcp-index.service` (`cheruby-docs index`), run by its
    timer on the schedule chosen in the panel, by "Run now", or by its path
    unit for the requests `run_index` leaves in `requests/`: walks the
    folders, updates the user's search index and writes
    `<user>.status.json` for the panel.
- **Code and secrets** in `/var/www/cheruby-docs-mcp`, root-owned, group
  read-only: `bin/cheruby-docs`, `bin/llama-server`, `pdfium/`, `model/`,
  `folders.json` (the folders and their filters), `docs.env` (root 600, the
  tokens) and `index.env` (root 600, no MCP token). Both hold the Nextcloud app
  password when a folder is in Nextcloud.
- **The index is the only state**, one SQLite file for the user in
  `/home/yunohost.app/cheruby-docs-mcp`, rebuilt from the folders, never
  backed up. Its `requests/` folder is the one place there the tool server
  writes. The settings are the rest: `docs_user`, each source's
  `<source>_enabled`, `_folders`, `_only` and `_skip`, `token_<user>`,
  `<source>_writable`, `nextcloud_account`, `nextcloud_password`,
  `index_schedule`, `index_calendar`, `index_quiet`, `embed_model`, `remote_*` and `whoopy_*`; configure renders `folders.json`, the
  env files and the units from them.
- **"Run now on the desktop"** has Whoopy's Ollama embed one run's
  passages, after checking its vectors agree with the Pi's; the timer
  never does. The key is pasted in the panel and goes to `index.env` only.
- **The token picks the user**, and the unit joins each folder's group to
  read it, and to change it when it is writable and mounted read-write,
  with everything else those groups could read hidden. Nextcloud's folders
  are read and written through Nextcloud over loopback, never on disk.

## Commands

Before committing:

```bash
(cd cheruby-docs && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo audit)
(cd cheruby-docs && cargo build --release)
uv venv .venv --python /usr/bin/python3 && uv pip install --python .venv/bin/python httpx
.venv/bin/python tests/check_server.py
for f in scripts/*; do bash -n "$f"; done
```

The offline check loads PDFium from `cheruby-docs/target/pdfium/`: unpack
the manifest's `pdfium` source there once.

A new binary: bump `version` in `cheruby-docs/Cargo.toml`, `cargo build
--release`, publish `target/release/cheruby-docs` as the asset
`cheruby-docs-aarch64-linux` of a release `cheruby-docs-<version>`, then put
its url and `sha256sum` in the manifest. The toolchain is rustup's, pinned
by `cheruby-docs/rust-toolchain.toml`.

## Packaging

The version is the `cheruby-docs/` crate's, then `~ynhN` for packaging
changes. The whole repository ships except `CLAUDE.md`, `BACKLOG.md`,
`TODO.md` and `cheruby-docs/target/`.
Backup holds the unit; the folders and tokens travel in the app settings.
Restore fetches the binary, PDFium, llama-server and the models again, so it
needs the network; the timer rebuilds the index.
