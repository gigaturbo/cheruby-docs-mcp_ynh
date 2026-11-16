# BACKLOG

A YunoHost app on the homelab that lets each user's AI agents, through the
Whoopy gateway, list, find, read and then search by meaning that user's own
folders, and write only where the user allows, without waking the desktop.

## Categories

| Letter | Covers |
|---|---|
| `M` | The MCP tools and what they can reach |
| `P` | The packaging, the lifecycle scripts and the sandbox |

## Bugs

Nothing open.

## Features

### F-P-3 · Whoopy access picked from the open folders

`wip` `small` `filed 2026-11-16`

"Whoopy access" takes typed labels, and a wrong one is refused at save.
The web admin shows that refusal only in its history strip, so the save
seems to do nothing.

**Do:** offer the open folders' labels as its choices, with a getter
defined for `show` only, as for Syncthing's folders. Built in 0.6.2, with
a saved label no longer open kept among the choices; owes T-P-3.

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

### T-P-3 · the Whoopy token, from the panel, stays out of the logs

`todo` `playtest` `filed 2026-11-10`

"Whoopy access" now offers the open folders as choices (F-P-3). Last result at `3df4e18` on 2026-11-16: pass.

## Closed

- `T-P-5` done `playtest` 2026-11-16 at `071063a` · on 0.6.1, a revoked app password: one index run noted it refused and asked Nextcloud nothing more, the panel said it was refused, Nextcloud's failed logins from 127.0.0.1 stayed at 0; a new password saved: the mark cleared, Notes read and indexed, the panel says it can be read and is writable; the new password in no new line of `yunohost-cli.log`, the save's operation log or the journal
- `T-P-1` done `playtest` 2026-11-16 at `bf59279` · on 0.6.1: 401 without a token; Nextcloud, Syncthing, the other Syncthing folders, `docs.env` and `/etc/shadow` out of the tool server's reach; its code and index read-only even to root; the embedding unit sees `bin/` and `model/` only, `/slots` 501; an index run without the old `RuntimeMaxSec` warning
- `B-M-2` closed 2026-11-16 · a revoked app password was tried again by the panel, the indexer and the server until Nextcloud held back every login from loopback; now one refusal stops the tries until a new password, and a 429 is named
- `B-P-1` closed 2026-11-16 · `RuntimeMaxSec` is ignored for a oneshot unit, so a stuck run blocked the timer; `TimeoutStartSec=6h` fails it, checked on a throwaway unit
- `T-P-4` done `playtest` 2026-11-16 at `c52e3c7` · its upgrade steps are moot, no old settings left; on a throwaway local folder: Syncthing's choices are its three folders, a 600 file counted as 1 unreadable, `*.pdf` left out hid a PDF from list, read, find and search while the index still held it, Documents' PDFs untouched, Local storage turned off took its folder out, the next run dropped it; "Nothing has changed" no longer holds, since the app password field always looks changed
- `B-P-2` closed 2026-11-16 · the panel said "could not be checked" for every folder on disk: `find` fails when it starts in YunoHost's root-only working folder; it now starts from `/`
- `T-M-2` done `playtest` 2026-11-16 at `3df4e18` · after a restore and a full rebuild, the same 5 of 7 documents in the top 5 as on 2026-11-11, the same two missed (F-M-12), 0.2 s a search
- `T-P-5` done `playtest` 2026-11-16 at `3df4e18` · an app password revoked in Nextcloud cut Notes off, and a new one pasted in the panel brought it back; neither app password in the new lines of `yunohost-cli.log` nor the 33 operation files since the restore
- `T-P-2` done `playtest` 2026-11-16 at `fbbb555` · nothing for `apt` to autoremove; backup, remove, restore without a warning: the user token, the gateway token and the Nextcloud app password back with the same fingerprints, and the 13 source, writable, ping, schedule and quiet-hour settings the same; both tokens list the two folders, Notes read through Nextcloud, no token gets 401; the timer back at its trimmed schedule, the index cleared for a rebuild; no secret in the new lines of `yunohost-cli.log`, the 6 operation logs or the commands' output
- `T-P-6` done `playtest` 2026-11-16 at `1fa9784` · "Custom" shows its field; `0 3 * * *` refused for the quiet hours 01-06 and `34 6-24 * * *` for hour 24; quiet hours over the current hour skipped it in the next run and made a start by hand log "no run now", while Run now ran; Rebuild stopped a run, dropped the working copy and kept search on the published index, then a run from zero indexed 312 files in 33 min; a stopped run counts as a success with `SuccessExitStatus=SIGTERM` and a failure without (a throwaway unit, since an unchanged run takes 1.3 s)
- `T-M-3` done `playtest` 2026-11-16 at `1fa9784` · with the Whoopy token both write tools offered on the two writable folders; in Nextcloud a folder made, a file created, replaced and read back, the old version kept whole; on disk, in a group-writable test folder, a new folder 2770 and file 660 in the folder's group, a file the group cannot change refused, one it can replaced keeping owner and mode, synced to two devices; `.pdf`, hidden names, `..`, unknown folders refused, and a Nextcloud label marked under Syncthing refused at save; inside the unit only Documents writable, even to root
- `T-P-1` done `playtest` 2026-11-16 at `1fa9784` · 401 without a token; the tool server sees only its code, its index and Documents, not Nextcloud, Syncthing, the other Syncthing folders, the env files, `settings.yml`, `/etc/shadow`, PHP sessions or `/mnt`; the embedding unit sees `bin/` and `model/` only, `/slots` 501; the indexer writes in its index folder only
- `T-M-1` done `spec` 2026-11-16 at `3df5d21` · `tests/check_server.py` 129 of 129, writes, quiet hours, refusals and throttling included
- `B-M-1` closed 2026-11-16 · four music scores past the model's 2048 tokens failed for good; a refused text is now embedded by its start, and a file failed by the embedding server retried, which indexed all four
- `F-M-11` done `large` 2026-11-16 · `write_file` and `create_folder` in the folders marked writable per source, never a delete, never through a link or a hard link, never the last 2 GB; disk folders mounted read-write only when writable; receive-only Syncthing folders and the multimedia and backup shares stay read-only; the panel says what cannot be changed, Nextcloud's through its permissions
- `F-P-1` done `medium` 2026-11-16 · index schedule as presets or cron, trimmed of quiet hours (01-06 by default) at which a run also stops; Index runs in the panel with its status, Run now and Rebuild
- `F-M-9` done `large` 2026-11-11 · a Nextcloud source read over WebDAV on loopback, PROPFIND and GET only, shares included; an app password made in Nextcloud and pasted in the panel, with an optional other login for read-only shares; the indexer lists only folders whose ETag changed and keeps what Nextcloud does not list; `Notes` moved over from Local storage
- `F-M-8` done `large` 2026-11-10 · one user, Syncthing and local sources with only/skip patterns, chosen in the config panel with a readability status per folder; links judged by where they lead; the old `folders` setting migrated on upgrade
- `F-M-7` done `medium` 2026-11-10 · one Whoopy token, for a user and some of their folders, shown in the config panel with its address and a "New token" button; it searches that user's index, its folders only; no token is traced any more, so none reaches `yunohost-cli.log`
- `F-M-5` done `large` 2026-11-09 · `search_documents` by meaning and keywords on an hourly index (llama-server built on the Pi, EmbeddingGemma 300M, numpy, FTS5): 288 files indexed in 31 min, 10 of 10 test documents in the top 5; healthchecks.io pinged; the model offered through Whoopy as `embeddinggemma` (Whoopy F-P-2)
- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
