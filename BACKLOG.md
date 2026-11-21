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

### B-P-3 · runs the tools ask for ping healthchecks.io like scheduled runs

`open` `low` `filed 2026-11-21`

A run started by `run_index` sends the same start and success pings as a
timer run. An agent calling it regularly keeps the check green while the
timer has stopped or every full run fails (R9's silent failure).

**Fix:** leave the pings out of a files-only run, with a marker the ping
unit can read, or ping a second check for those runs.

## Features

### F-P-6 · one Rust binary for the tool server, the indexer and their commands

`wip` `large` `filed 2026-11-21`

The tool server, the indexer and the text readers ran as Python from a uv
venv, as Whoopy's daemon did before its own binary. They are ported to
`cheruby-docs/` with the same index format: on the author's Documents, 401
files with the same status and identical passages, 17 s against 39 s with a
stand-in embedder.

**Do:** publish the release `cheruby-docs-0.11.0`, pin its sha256, upgrade
(ask first), then the reopened checks.

### F-M-17 · an agent asks for an index run

`wip` `medium` `filed 2026-11-21`

A file written with `write_file` could not be found by search until the next
timer run, up to an hour later. `run_index` leaves a request that the index's
path unit starts a run for, of some files or of everything; refused in the
quiet hours, past 50 files and once 20 requests wait.

**Do:** T-M-5 on the installed app.

### F-M-6 · read scans and images into the search index

`todo` `medium` `filed 2026-11-09`

After F-M-5, the 50 PDFs without a text layer and the images are listed as
waiting but cannot be found by meaning.

**Do:** a nightly timer sends the waiting pages to qwen3-vl on the desktop
through Whoopy on loopback, with a key of its own allowed that model only;
one wake a night, none when nothing waits. Pages rendered with PDFium,
which the binary already loads (`pdfium-render`). The read text is kept by file fingerprint, so a new
embedding model does not redo it.

**Models to compare (2026-11-16):** qwen3-vl and qwen3.5-9B through
Ollama, on the same pages, by the text read. jina-ocr-v1 only if both read
poorly: it is not in Ollama (Transformers with `trust_remote_code`, or
vLLM, which needs a newer GPU than the 1080 Ti), its weights are bf16,
which Pascal cannot run natively (about 6.7 GB in fp16), and its licence is
CC-BY-NC-4.0.

## Tests

### T-M-5 · an agent's run_index reaches the index and nothing else

`todo` `playtest` `filed 2026-11-21`

New with F-M-17: the tool server writes in the index's `requests/` folder,
and a path unit starts the index unit.

### T-M-2 · search finds documents by meaning

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: search and the passages are now the binary's. Last done
2026-11-18 at `5d3644a`: 16 of 17 in the top 5, mean reciprocal rank 0.88,
3.6 s a search.

### T-P-1 · the installed server reads only its folders

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6 and F-M-17: new binaries under the same sandbox, and a
writable `requests/` bind inside the read-only index folder. Last done
2026-11-18 at `5d3644a`: 401 without a token, the env files and
`/etc/shadow` out of reach, code and index read-only.

### T-P-2 · backup and restore keep the tokens

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: restore now fetches the binary and PDFium. Last done
2026-11-18 at `8b0a4db`: 46 s, the 46 settings and both tokens the same, no
secret in the logs.

### T-P-3 · the Whoopy token, from the panel, stays out of the logs

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: the server's restarts wait for the binary's start line.
Last done 2026-11-18 at `8b0a4db`: the old token refused with 401, the new
one in no log.

### T-P-4 · the upgrade keeps the folders, and the panel's sources choose them

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: Syncthing's folders and the panel's status now come from
the binary. Last done 2026-11-18 at `8b0a4db`: the sources, the filters and
"Nothing has changed" as expected.

### T-P-5 · Nextcloud folders are read through Nextcloud only

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: a new WebDAV client. Last done 2026-11-16 at `071063a`:
a revoked app password asked once, then not again until a new one.

### T-M-3 · writes land in the writable folders only

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: the writes are the binary's. Last done 2026-11-16 at
`1fa9784`: in Nextcloud and on disk, 660 and 2770 in the folder's group,
every escape refused.

### T-M-4 · a first index embedded on the desktop agrees with the Pi

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: the remote run and its fallback are the binary's. Last
done 2026-11-19 at `116a392`: 333 files in 3 min 47 s, the same search,
the Pi taking over when Whoopy stopped.

### T-P-6 · the index runs on the chosen schedule, from the panel

`todo` `playtest` `filed 2026-11-21`

Reopened by F-P-6: the schedule and the status come from the binary. Last
done 2026-11-16 at `1fa9784`: presets, custom schedules and quiet hours as
expected, Rebuild keeping search on the published index.

## Closed

- `T-M-1` done `spec` 2026-11-21 at `d9ba5fb` · `tests/check_server.py` 177 of 177 against the binary, `run_index` included; the 6 checks of a run's inside moved to `cargo test`, 22 of 22
- `F-P-5` done `medium` 2026-11-19 · the redrawn icon set from ynh-app-icons 0ca6205: `conf/logo.png` and `doc/logo.svg` replaced, `logo_update/` deleted (0.10.1)
- `F-M-10` done `medium` 2026-11-19 · "Run now on the desktop" embeds a run through Whoopy's `ollama-embed/` route (Whoopy F-P-3) with a pasted key, after 20 check passages agree to 0.995 (0.9997 measured); a Whoopy error sends the rest of the run to the Pi; never used by the timer
- `F-P-2` done `medium` 2026-11-18 · embedding model chosen in the panel (EmbeddingGemma 300M, jina-embeddings-v5-text-nano, Qwen3-Embedding 0.6B, each Q8 or Q4), pinned by commit and sha256, prompts per family, a change downloads first and re-indexes; scored on 17 queries with the time of a full index: EmbeddingGemma Q8 16/17, MRR 0.88, 38 min (kept as default); EmbeddingGemma Q4 16/17, MRR 0.81, 36 min; jina Q8 16/17, MRR 0.85, 29 min; Qwen3 Q8 15/17, MRR 0.79, 112 min.
- `F-P-4` done `medium` 2026-11-18 · llama-server fetched as release `llama-server-b12223`, built by `build/llama-server.sh` in `.github/workflows/llama-server.yml` (debian:12, arm64, Cortex-A76 flags, no web UI) and pinned by sha256: nothing compiled at install, upgrade or restore, `libgomp1` the only apt dependency; on 162 passages the same vectors bit for bit as the Pi's native build, at the same speed (61 to 67 s both); the upgrade to 0.9.0 took 8 min, then the index was rebuilt once for the new binary, 326 indexed in 38 min.
- `F-M-15` done `medium` 2026-11-17 · PDFs read with pypdfium2, not pypdf: on 314 PDFs 4 s against 45 s, 1 failure against 2, the same text order, ligatures as letters, but 468 unreadable characters in 11 files against 280 in 9; words cut at a line end joined; a PDF's running headers and footers indexed once, a number matching only at a fixed distance from the page number (420 of 31,928 lines, 30 files, 9% at most); HTML and XML indexed by their text; NFKC left out, PDFium already gives letters. After a full rebuild (35 min, 315 indexed, 1 password-protected PDF failed): 16 of 17 in the top 5, mean reciprocal rank 0.84 against 0.88, 3.7 s a search; the Dell invoice 4th, not 1st, its page now read from the payment terms, under a 2013 computer order form
- `T-M-2` done `playtest` 2026-11-17 at `dddd300` · on 0.8.0 with F-M-16, after a run that indexed the user's one ods (7 passages) and one xlsx (1): the saved set 16 of 17 in the top 5, mean reciprocal rank 0.88, 3.7 s a search, 4.3 s at most, as after F-M-14; the ods 4th for a query in other words; the xlsx, a table of "HP", "HC" and numbers named ELEC/Feuil1, not in the top 5 for two, too little text to match by meaning. `read_file` gave both, sheets as headings
- `F-M-16` done `small` 2026-11-17 · pptx, odp, xlsx, ods, epub and eml read by `read_file` and the index, from their XML or with `email`, no new wheel, in a new `docs_text.py` with PDF and Word; slides and sheets as headings, in the document's order, notes left out, 20 MB unpacked at most. On the server, 11 odp, 7 ods, 1 pptx and 1 xlsx all read, 0.08 s at most; no epub or eml there to try
- `F-M-14` done `medium` 2026-11-17 · passages up to 900 characters, a file or section that fits kept whole, a longer one cut at its subheadings then paragraphs, each starting with `<file> > <heading>`; Word and ODT headings as `#`; `passages-2` joins the model in `meta`. On 17 queries (4 arXiv papers added in `Documents/Research`): MRR 0.88 against 0.90, 16 of 17 in the top 5 both, the answer in the top passage 6 of 9 both, passages 40% shorter; a section always cut on its own put the wool note 4th, so whole-when-it-fits was kept. Shipped on a tie, for the shorter passages
- `T-P-1` done `playtest` 2026-11-16 at `f2cab55` · on 0.7.0: 401 without a token; the tool server sees Documents only under Syncthing, not Nextcloud, PHP sessions, the env files or `/etc/shadow`, its code and index read-only; the embedding and reranking units see `bin/` and `model/` only, `/slots` 501 on both
- `T-M-2` done `playtest` 2026-11-16 at `f2cab55` · on 0.7.0 with `tests/eval_search.py`: 9 of 9 in the top 5, mean reciprocal rank 0.87, 3.8 s a search, 4.1 s at most, against 7 of 9, 0.71 and 0.1 s before
- `F-M-12` done `medium` 2026-11-16 · the fused top 30, by file name and first 300 characters, reordered by jina-reranker-v2-base-multilingual (CC-BY-NC-4.0) in its own unit, the best passage per file; bge-reranker-v2-m3 (8 of 9, 12 s) and Qwen3-Reranker-0.6B (8 of 9, 37 s) measured and set aside; the fused order kept, and said, when it fails
- `F-M-13` done `small` 2026-11-16 · `tests/eval_search.py` scores `search_documents` on a running server against a private query set, rank and time per query, top 5 count and mean reciprocal rank
- `F-P-3` done `small` 2026-11-16 · "Whoopy access" picked from the open folders, a saved label no longer open kept among the choices to be removed
- `T-P-1` done `playtest` 2026-11-16 at `bf59279` · on 0.6.1: 401 without a token; Nextcloud, Syncthing, the other Syncthing folders, `docs.env` and `/etc/shadow` out of the tool server's reach; its code and index read-only even to root; the embedding unit sees `bin/` and `model/` only, `/slots` 501; an index run without the old `RuntimeMaxSec` warning
- `B-M-2` closed 2026-11-16 · a revoked app password was tried again by the panel, the indexer and the server until Nextcloud held back every login from loopback; now one refusal stops the tries until a new password, and a 429 is named
- `B-P-1` closed 2026-11-16 · `RuntimeMaxSec` is ignored for a oneshot unit, so a stuck run blocked the timer; `TimeoutStartSec=6h` fails it, checked on a throwaway unit
- `B-P-2` closed 2026-11-16 · the panel said "could not be checked" for every folder on disk: `find` fails when it starts in YunoHost's root-only working folder; it now starts from `/`
- `T-M-2` done `playtest` 2026-11-16 at `3df4e18` · after a restore and a full rebuild, the same 5 of 7 documents in the top 5 as on 2026-11-11, the same two missed (F-M-12), 0.2 s a search
- `T-P-5` done `playtest` 2026-11-16 at `3df4e18` · an app password revoked in Nextcloud cut Notes off, and a new one pasted in the panel brought it back; neither app password in the new lines of `yunohost-cli.log` nor the 33 operation files since the restore
- `T-P-1` done `playtest` 2026-11-16 at `1fa9784` · 401 without a token; the tool server sees only its code, its index and Documents, not Nextcloud, Syncthing, the other Syncthing folders, the env files, `settings.yml`, `/etc/shadow`, PHP sessions or `/mnt`; the embedding unit sees `bin/` and `model/` only, `/slots` 501; the indexer writes in its index folder only
- `B-M-1` closed 2026-11-16 · four music scores past the model's 2048 tokens failed for good; a refused text is now embedded by its start, and a file failed by the embedding server retried, which indexed all four
- `F-M-11` done `large` 2026-11-16 · `write_file` and `create_folder` in the folders marked writable per source, never a delete, never through a link or a hard link, never the last 2 GB; disk folders mounted read-write only when writable; receive-only Syncthing folders and the multimedia and backup shares stay read-only; the panel says what cannot be changed, Nextcloud's through its permissions
- `F-P-1` done `medium` 2026-11-16 · index schedule as presets or cron, trimmed of quiet hours (01-06 by default) at which a run also stops; Index runs in the panel with its status, Run now and Rebuild
- `F-M-9` done `large` 2026-11-11 · a Nextcloud source read over WebDAV on loopback, PROPFIND and GET only, shares included; an app password made in Nextcloud and pasted in the panel, with an optional other login for read-only shares; the indexer lists only folders whose ETag changed and keeps what Nextcloud does not list; `Notes` moved over from Local storage
- `F-M-8` done `large` 2026-11-10 · one user, Syncthing and local sources with only/skip patterns, chosen in the config panel with a readability status per folder; links judged by where they lead; the old `folders` setting migrated on upgrade
- `F-M-7` done `medium` 2026-11-10 · one Whoopy token, for a user and some of their folders, shown in the config panel with its address and a "New token" button; it searches that user's index, its folders only; no token is traced any more, so none reaches `yunohost-cli.log`
- `F-M-5` done `large` 2026-11-09 · `search_documents` by meaning and keywords on an hourly index (llama-server built on the Pi, EmbeddingGemma 300M, numpy, FTS5): 288 files indexed in 31 min, 10 of 10 test documents in the top 5; healthchecks.io pinged; the model offered through Whoopy as `embeddinggemma` (Whoopy F-P-2)
- `F-M-4` done `large` 2026-11-09 · list, find and read the author's folders through Whoopy; Claude Code and Hermes Agent each found and read a document with a key of their own
