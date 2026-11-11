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

### B-P-1 · a stuck index run is never stopped

`open` `medium` `filed 2026-11-10`

`index.service` is `Type=oneshot` with `TimeoutStartSec=infinity`, and
systemd ignores `RuntimeMaxSec=6h` for a oneshot unit (`systemd-analyze
verify` says so on systemd 252). A run that hangs blocks the timer for good,
while the unit's comment says it fails after 6 hours.

**Fix:** `TimeoutStartSec=6h` and no `RuntimeMaxSec`.

## Features

### F-P-1 · indexing controls and status in the panel

`todo` `medium` `filed 2026-11-10`

The index runs hourly with no way to start, rebuild, slow or stop it, and
its progress is only in the journal.

**Do:** a schedule (off, hourly, every 6 h, nightly) that writes the timer's
`OnCalendar`; a "Run now" button (start without waiting); a "Rebuild" button
(drop the index, the next run starts from zero); a read-only status from a
file the indexer writes: last run, files indexed, waiting and failed.

### F-M-10 · first index embedded through Whoopy on the desktop's GPU

`todo` `medium` `filed 2026-11-10` `needs: F-P-1`

A first index of many files takes hours on the Pi, and nearly all of it is
embedding; reading the text is fast.

**Do:** a "Run now on remote" button: the indexer reads the files on the
Pi and sends the passages to Whoopy's Ollama `embeddinggemma`, with a key
allowed that model only. Before using it, it embeds a fixed set of about 20
passages on both servers and refuses the remote unless every pair has a
cosine of 0.995 or more, since search still embeds queries on the Pi. When
Whoopy cannot be reached, the run goes on locally. Never used by the timer.

### F-P-2 · choice of embedding model in the panel

`todo` `medium` `filed 2026-11-10`

The model is fixed to EmbeddingGemma 300M.

**Do:** a short list in the panel, EmbeddingGemma by default, each model
pinned in the manifest by commit and sha256 so a restore can fetch it, and
its search prefixes kept in the code. A change downloads the model,
restarts the embedding unit and re-indexes everything (`DOCS_EMBED_MODEL`).
F-M-10's remote must serve the same model.

### F-M-11 · write files in the folders marked writable

`todo` `large` `filed 2026-11-10` `needs: F-M-9`

The tools only read.

**Do:** a "Writable folders" field per source, and a write tool for those
folders only, never a delete. Disk folders become writable mounts in the unit, group-writable with setgid directories;
a Syncthing folder needs file versioning on before it can be `rw`, since a
change syncs to every device. Nextcloud writes go through WebDAV `PUT`.

### F-M-6 · read scans and images into the search index

`todo` `medium` `filed 2026-11-09`

After F-M-5, the 50 PDFs without a text layer and the images are listed as
waiting but cannot be found by meaning.

**Do:** a nightly timer sends the waiting pages to qwen3-vl on the desktop
through Whoopy on loopback, with a key of its own allowed that model only;
one wake a night, none when nothing waits. Pages rendered with `pypdfium2`
from the locked wheels. The read text is kept by file fingerprint, so a new
embedding model does not redo it.

### F-M-12 · rank search results by how well they answer the query

`todo` `medium` `filed 2026-11-11`

Search fuses the embedding ranking and the keyword ranking (RRF), and
nothing judges the passages against the query itself. T-M-2 on 2026-11-11
found 5 of 7 documents in the top 5: the CVs came above the publications
list for "list of scientific papers I authored", and the wool section of
the washing note never came up for "how to wash wool by hand". Both files
are indexed.

**Do:** a ranking step on the fused top 30 or so, before the limit is
applied, run on the Pi on loopback like the embedding unit. Pass: T-M-2's
seven queries all in the top 5, and a search still answers in a few seconds.

## Tests

## Closed

- `T-P-3` done `playtest` 2026-11-11 at `f3a3628` · narrowed in the panel to two folders then to `Notes`, then a new token: it lists, finds and searches `Notes` only and is told `Documents` does not exist; the token from before gets 401; no gateway token, user token or Nextcloud password in the 50 new lines of `yunohost-cli.log` or the 4 operations' logs
- `T-P-1` done `playtest` 2026-11-11 at `f3a3628` · 401 without the token; with it, Documents and Notes listed and read, `..` refused; the tool server sees in `/var/www` its own code only, in `/home/yunohost.app` its index (read-only) and the Syncthing folder, and cannot read `settings.yml`, `docs.env`, `index.env` or `/etc/shadow`; the embedding unit sees `bin/` and `model/` only, `/slots` answers 501; the indexer writes in its index folder only
- `T-M-2` done `playtest` 2026-11-11 at `f3a3628` · after a full rebuild, 5 of 7 documents in the top 5, French and English: the washing recipe, the btrfs note, the Dell invoice, the garden-centre CV and the bread note; missed: the publications list (CVs ranked above it) and the wool section of the washing note
- `T-P-2` done `playtest` 2026-11-11 at `ee29bbd` · backup, remove, restore: the user token, the gateway token and the Nextcloud app password came back with the same fingerprints, the ping and source settings the same, the service still without `nextcloud`; no token answered 401; no secret in the new lines of `yunohost-cli.log` or the backup, remove and restore logs; old kernel kept with `apt-mark manual`. Notes then failed: the app password had been revoked in Nextcloud meanwhile
- `T-P-5` done `playtest` 2026-11-11 at `ee29bbd` · the service no longer joins `nextcloud`; a pasted app password saved and a wrong one refused at save; list, find, read and search on `Notes` through Nextcloud, one PROPFIND for an unchanged account (the `occ` build passed the same at `b7823bb`); the password in no new line of `yunohost-cli.log`, the operation logs or the journal
- `F-M-9` done `large` 2026-11-11 · a Nextcloud source read over WebDAV on loopback, PROPFIND and GET only, shares included; an app password made in Nextcloud and pasted in the panel, with an optional other login for read-only shares; the indexer lists only folders whose ETag changed and keeps what Nextcloud does not list; `Notes` moved over from Local storage
- `T-P-4` done `playtest` 2026-11-10 at `4a7b472` · the upgrade made Documents a Syncthing folder and Notes a local one, dropped `folders` and `whoopy_user`, kept the gateway token and its two folders; the panel listed Syncthing's three folders as choices and counted a 600 file as unreadable; `*.pdf` left out hid PDFs from list, read and a search whose index still held them; local off removed Notes; an unchanged save restarted nothing
- `F-M-8` done `large` 2026-11-10 · one user, Syncthing and local sources with only/skip patterns, chosen in the config panel with a readability status per folder; links judged by where they lead; the old `folders` setting migrated on upgrade
- `F-M-7` done `medium` 2026-11-10 · one Whoopy token, for a user and some of their folders, shown in the config panel with its address and a "New token" button; it searches that user's index, its folders only; no token is traced any more, so none reaches `yunohost-cli.log`
- `F-M-5` done `large` 2026-11-09 · `search_documents` by meaning and keywords on an hourly index (llama-server built on the Pi, EmbeddingGemma 300M, numpy, FTS5): 288 files indexed in 31 min, 10 of 10 test documents in the top 5; healthchecks.io pinged; the model offered through Whoopy as `embeddinggemma` (Whoopy F-P-2)
- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
- `T-M-1` done `spec` 2026-11-10 · `tests/check_server.py`: 66 of 66 pass on the locked wheels, search, indexer, filters and links to filtered paths included
