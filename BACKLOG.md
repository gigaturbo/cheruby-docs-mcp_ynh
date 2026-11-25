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

### B-M-3 · a file once opened in Nextcloud's Text refuses writes for days

`open` `medium` `filed 2026-11-25`

17 `write_file` calls into `Notes` were refused since 2026-11-18 with
"Nextcloud answered 423." Text takes a files_lock app lock on each file
opened in it and, in Text 8.0.0, keeps it when the session ends without a
clean close: 10 locks held on 2026-11-25, the oldest since 2026-09-11
(nextcloud/text#9042). Not this app's doing, and no client can write past
an app lock. The package now names the Text editor in the refusal.

**Fix:** upstream, in Text 9.0.3 (NC 35.0.4; 8.0.2 in NC 34.0.7), whose
repair step releases locks with no open session. Nextcloud 35.0.3, installed
on 2026-11-25, has Text 9.0.2, still without it. Until then, ask first, then
per file nobody has open: `occ files:lock --unlock <file id> <user>`. Close
once 35.0.4 is installed and `oc_files_lock` holds no `text` lock older than
a day.

2026-11-25: the ten Text locks released (no session for 48 minutes or
more), then a `write_file` of `INDEX.md`'s own text passed. The 423 message
was seen on it before. A manual lock of 2026-09-22 on a file since deleted
(id 1718) is kept: not Text's.

## Features

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

**Last:** 2026-11-21 at `940adcd`, on 0.11.0: `run_index` of one PDF answered "indexed" in 1 s and ran that file only; `..` refused; inside the server's namespace only `requests/` writable; the path unit never restarted the indexer.

Reopened by B-P-3: the index unit's first step decides a tools' run, and a request waiting at a timer run is now answered by the full run.

2026-11-25 at `5b8fad7`, 0.12.1~ynh1 installed: `run-index` of one note skipped `ping@start` and `ping@0` on their condition and left `tools-run`; Run now then sent both pings and dropped it. The rest of the check not run.

**Check:** the skill's T-M-5, plus: after a `run_index`, `journalctl -u 'cheruby-docs-mcp-ping@*'` shows the ping units skipped on their condition, and healthchecks.io logged nothing; "Run now" still pings start and success.

### T-P-6 · the index runs on the chosen schedule, from the panel

`todo` `playtest` `filed 2026-11-21`

**Last:** 2026-11-21 at `ed08fb3`: every schedule and quiet hours followed, Run now, Rebuild (B-P-4 found).

Reopened by B-P-4: the status's next run.

**Check:** the skill's T-P-6, plus: during an hourly run, `sudo cheruby-docs-mcp status` and the panel say "Next run: after this run."; with the schedule never, "none: the schedule is never."

### T-P-1 · the installed server reads only its folders

`todo` `playtest` `filed 2026-11-16`

**Last:** 2026-11-25 at `b649caf`, on 0.12.0~ynh1: the loopback table loaded, the units' ports only.

Reopened by B-P-3: `conf/index.service` and `conf/ping@.service` changed.

**Check:** the skill's T-P-1, and the index unit's first step runs in its sandbox as the app.

### T-P-3 · the Whoopy token, from the panel and the command, stays out of the logs

`todo` `playtest` `filed 2026-11-22`

**Last:** 2026-11-25 at `b649caf`, 0.12.0~ynh1 installed: `mcp_token`
kept its fingerprint, neither token in the upgrade's lines of
`yunohost-cli.log` or its operation logs. The new wording (F-P-8) not seen
in the panel, and `token --new` still not run: it breaks Whoopy's `docs`
server until the new token is pasted in Whoopy's admin page.
Before: 2026-11-22 at `940062b`, `whoopy_token` gone, the command showed
both tokens and refused `serve`, Whoopy's `/mcp/docs` listed seven tools.

Reopened by F-P-7: the token's setting, panel section and rotation path
changed. Last done 2026-11-21 at `ed08fb3`: Whoopy access narrowed to two folders then one, New
token: the new token saw Documents only, the old one 401; neither token in
`yunohost-cli.log` or the operation logs from this app.

**Check:** after the upgrade, `sudo cheruby-docs-mcp token` shows the same
Whoopy token as before and Whoopy's `/mcp/docs` still answers;
`sudo cheruby-docs-mcp token --new` prints no token, the old one gets 401,
the new one 200 once pasted in Whoopy; no token in `yunohost-cli.log`; the
panel shows "MCP token"; `sudo cheruby-docs-mcp status` answers as the
binary did.

### T-P-2 · backup and restore keep the tokens

`todo` `playtest` `filed 2026-11-22`

Reopened by F-P-7: restore now moves an old `whoopy_token` to `mcp_token`.
Last done 2026-11-21 at `ed08fb3`: backup 2 s, remove, restore 37 s; the 38 settings and both
tokens the same and answering; no secret in the operation files or the CLI
log.

**Check:** a backup made before the upgrade restores with the same Whoopy
token under `mcp_token` and no `whoopy_token`; `/usr/local/bin/cheruby-docs-mcp`
there after restore, gone after remove.

## Closed

- `B-P-4` closed 2026-11-25 · during a run the panel said "the schedule is never." on an hourly schedule; it now says "after this run" while the timer is on
- `B-P-3` closed 2026-11-25 · a run `run_index` started pinged healthchecks.io like a timer run, so an agent could keep the check green; such a run now pings nothing
- `T-M-3` done `playtest` 2026-11-25 at `b649caf` · on disk (Syncthing Documents) and in Nextcloud (Notes): a file moved, a folder renamed, a move onto a name, into itself, into a missing folder and across folders refused; moved files kept 660, folders 770, the folder's group; a Text-locked file refused with the Text message and left unchanged; test files deleted
- `F-M-19` done 2026-11-25 · `move_file` moves or renames a file or a folder within one writable folder, on disk and in Nextcloud, never replacing, never across a share, never changing what the filters keep
- `T-M-1` done `spec` 2026-11-25 at `1d71e1a` · `tests/check_server.py` 206 of 206 against the binary, `move_file` and the Text lock included; `cargo test` 23 of 23
- `B-P-5` closed 2026-11-25 · the four units could reach every loopback service, PostgreSQL, LDAP, the YunoHost API and Redis among them, over IPv4 and `::1`
- `F-P-8` done 2026-11-25 · the README, `doc/mcp.md`, the config panel, the New token message and `token --new` say to add the server `docs` on Whoopy's status page and to change its token in Whoopy's admin page
- `F-P-7` done 2026-11-22 · `sudo cheruby-docs-mcp token [--new]` and the gateway token named `mcp_token`, as in the other MCP apps; the read and status commands pass to the binary (0.11.0~ynh2, installed 2026-11-22)
- `F-M-18` done `small` 2026-11-22 · each tool, its arguments and its limits in `doc/mcp.md`, in the layout the other MCP apps share; the README keeps a short list and the commands name the tool each mirrors; `search_documents`' `before` documented as inclusive
- `T-M-2` done `playtest` 2026-11-21 at `ed08fb3` · on 0.11.0: 16 of 17 in the top 5, mean reciprocal rank 0.88, 4.1 s at most, 3.5 s a search, as on 2026-11-18; the publications list still missed; the same on an index embedded on the desktop
- `T-P-4` done `playtest` 2026-11-21 at `ed08fb3` · Syncthing's choices its three folders; a mode 600 file counted 1; `*.pdf` left out hid the PDFs from listing, reading, finding and search at once; Local storage on then off brought its folder and took it away; an unchanged save restarted nothing, saying "Config updated" since the password field always looks changed
- `T-P-5` done `playtest` 2026-11-21 at `ed08fb3` · no `nextcloud` group; a wrong app password refused at save, with no failed login counted; a PDF in Notes read, found and searched; an unchanged run sent one PROPFIND; a revoked password refused once each by the server, the indexer and the panel, then nothing sent, the panel naming it; a new one brought Notes back; no password in the logs. No share from another account to try
- `T-M-4` done `playtest` 2026-11-21 at `ed08fb3` · 340 files indexed on the desktop in 2 min, the same search as the Pi's index; Whoopy stopped 30 s into a run: the run went on on the Pi and ended done in 36 min, no file failed for it; the key in no log. The Whoopy key first allowed only `ollama/...` and got 403, the run then on the Pi as it should
- `F-M-17` done `medium` 2026-11-21 · `run_index` asks the index unit through a request in `requests/` and a path unit, for some files or everything; each request answered in the status file; refused in the quiet hours, past 50 files or 20 waiting requests, for one waiting 5 minutes, and for everything within 15 minutes of the last full run (0.11.0)
- `F-P-6` done `large` 2026-11-21 · the tool server, the indexer and their commands one Rust binary, release `cheruby-docs-0.11.0`, with PDFium chromium/8076, the build pypdfium2 shipped; the same passages as the Python on 401 files; the upgrade kept the index (model, 4,833 passages, 428 files unchanged) and search answered by meaning in 3 s (0.11.0)
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
