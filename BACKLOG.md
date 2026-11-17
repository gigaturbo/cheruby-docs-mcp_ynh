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
The remote must serve the model chosen in the panel (F-P-2): Ollama has
`embeddinggemma` and `qwen3-embedding:0.6b`, not jina-embeddings-v5-text-nano.

### F-P-2 · choice of embedding model in the panel

`todo` `medium` `filed 2026-11-10`

The model is fixed to EmbeddingGemma 300M.

**Do:** a short list in the panel, EmbeddingGemma by default, each model
pinned in the manifest by commit and sha256 so a restore can fetch it, and
its search prefixes kept in the code. A change downloads the model,
restarts the embedding unit and re-indexes everything (`DOCS_EMBED_MODEL`).
F-M-10's remote must serve the same model.

**Candidate (2026-11-16):** jina-embeddings-v5-text-nano-retrieval (212M,
CC-BY-NC-4.0), smaller than EmbeddingGemma, last-token pooling on upstream
llama.cpp. Scored with F-M-13 against EmbeddingGemma, with the time of a
full index. v5-text-small (677M) would double the index time; v5-omni
embeds images but gives no text to search or read.

### F-M-6 · read scans and images into the search index

`todo` `medium` `filed 2026-11-09`

After F-M-5, the 50 PDFs without a text layer and the images are listed as
waiting but cannot be found by meaning.

**Do:** a nightly timer sends the waiting pages to qwen3-vl on the desktop
through Whoopy on loopback, with a key of its own allowed that model only;
one wake a night, none when nothing waits. Pages rendered with `pypdfium2`
from the locked wheels. The read text is kept by file fingerprint, so a new
embedding model does not redo it.

**Models to compare (2026-11-16):** qwen3-vl and qwen3.5-9B through
Ollama, on the same pages, by the text read. jina-ocr-v1 only if both read
poorly: it is not in Ollama (Transformers with `trust_remote_code`, or
vLLM, which needs a newer GPU than the 1080 Ti), its weights are bf16,
which Pascal cannot run natively (about 6.7 GB in fp16), and its licence is
CC-BY-NC-4.0.

## Tests

Nothing open.

## Closed

- `F-M-15` done `medium` 2026-11-17 · PDFs read with pypdfium2, not pypdf: on 314 PDFs 4 s against 45 s, 1 failure against 2, the same text order, ligatures as letters, but 468 unreadable characters in 11 files against 280 in 9; words cut at a line end joined; a PDF's running headers and footers indexed once, a number matching only at a fixed distance from the page number (420 of 31,928 lines, 30 files, 9% at most); HTML and XML indexed by their text; NFKC left out, PDFium already gives letters. After a full rebuild (35 min, 315 indexed, 1 password-protected PDF failed): 16 of 17 in the top 5, mean reciprocal rank 0.84 against 0.88, 3.7 s a search; the Dell invoice 4th, not 1st, its page now read from the payment terms, under a 2013 computer order form
- `T-M-2` done `playtest` 2026-11-17 at `dddd300` · on 0.8.0 with F-M-16, after a run that indexed the user's one ods (7 passages) and one xlsx (1): the saved set 16 of 17 in the top 5, mean reciprocal rank 0.88, 3.7 s a search, 4.3 s at most, as after F-M-14; the ods 4th for a query in other words; the xlsx, a table of "HP", "HC" and numbers named ELEC/Feuil1, not in the top 5 for two, too little text to match by meaning. `read_file` gave both, sheets as headings
- `F-M-16` done `small` 2026-11-17 · pptx, odp, xlsx, ods, epub and eml read by `read_file` and the index, from their XML or with `email`, no new wheel, in a new `docs_text.py` with PDF and Word; slides and sheets as headings, in the document's order, notes left out, 20 MB unpacked at most. On the server, 11 odp, 7 ods, 1 pptx and 1 xlsx all read, 0.08 s at most; no epub or eml there to try
- `F-M-14` done `medium` 2026-11-17 · passages up to 900 characters, a file or section that fits kept whole, a longer one cut at its subheadings then paragraphs, each starting with `<file> > <heading>`; Word and ODT headings as `#`; `passages-2` joins the model in `meta`. On 17 queries (4 arXiv papers added in `Documents/Research`): MRR 0.88 against 0.90, 16 of 17 in the top 5 both, the answer in the top passage 6 of 9 both, passages 40% shorter; a section always cut on its own put the wool note 4th, so whole-when-it-fits was kept. Shipped on a tie, for the shorter passages
- `T-P-1` done `playtest` 2026-11-16 at `f2cab55` · on 0.7.0: 401 without a token; the tool server sees Documents only under Syncthing, not Nextcloud, PHP sessions, the env files or `/etc/shadow`, its code and index read-only; the embedding and reranking units see `bin/` and `model/` only, `/slots` 501 on both
- `T-M-2` done `playtest` 2026-11-16 at `f2cab55` · on 0.7.0 with `tests/eval_search.py`: 9 of 9 in the top 5, mean reciprocal rank 0.87, 3.8 s a search, 4.1 s at most, against 7 of 9, 0.71 and 0.1 s before
- `F-M-12` done `medium` 2026-11-16 · the fused top 30, by file name and first 300 characters, reordered by jina-reranker-v2-base-multilingual (CC-BY-NC-4.0) in its own unit, the best passage per file; bge-reranker-v2-m3 (8 of 9, 12 s) and Qwen3-Reranker-0.6B (8 of 9, 37 s) measured and set aside; the fused order kept, and said, when it fails
- `F-M-13` done `small` 2026-11-16 · `tests/eval_search.py` scores `search_documents` on a running server against a private query set, rank and time per query, top 5 count and mean reciprocal rank
- `T-P-3` done `playtest` 2026-11-16 at `40e8365` · on 0.6.2, "Whoopy access" offers Documents and Notes; Notes alone saved and Whoopy saw only Notes; cleared, all folders open again
- `F-P-3` done `small` 2026-11-16 · "Whoopy access" picked from the open folders, a saved label no longer open kept among the choices to be removed
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
