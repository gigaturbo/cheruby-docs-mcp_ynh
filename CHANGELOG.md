# Changelog

## 0.15.1~ynh1

- A word whisper cut between two parts of a transcript now reads as one word ("factures", no longer "fact ures").
- A recording transcribed again after the model is changed is transcribed with the new model, no longer given the old model's text.

## 0.15.0~ynh1

- `transcribe_file`: an agent gets the text of a recording in the user's folders, with its times, then cleans it, saves it with `write_file` and moves the recording with `move_file`. whisper small or large-v3-turbo runs on the server; a long recording is done in parts, and a call again later gets the rest.
- The desktop's GPU can transcribe instead, through Whoopy, never, on request or always, chosen in the config panel under "Speech to text". It needs Whoopy's `speaches/...` route and a key allowed it.
- The page's new Transcribe tab does the same for a recording in a folder or one sent from the browser, up to 500 MB.
- The package now installs ffmpeg, and its memory needs grow by about 0.6 GB with whisper small, 1.2 GB with large-v3-turbo.

## 0.14.0~ynh1

- The page's new Config tab sets the index schedule, its quiet hours and each source's "Only these files" and "Leave out". A change applies at once, with no restart. The upgrade moves them there from the config panel, into `/var/lib/cheruby-docs-mcp/settings.json`, which backups now carry.
- The tool server runs the index schedule itself, and the index timer is gone. A run missed while the server was stopped starts when it is back.
- healthchecks.io is pinged when any index run fails, and on the success of a run the schedule or Run now started. It is no longer pinged when a run starts. Keep the check's period at the schedule's.

## 0.13.1~ynh1

- The page shows the new cherry mascot and its favicons, and its green is a darker leaf green, no longer lime.

## 0.13.0~ynh2

- The page answered 404: its address in nginx was the last folder's path, not the app's.

## 0.13.0~ynh1

- A page for the docs user, behind the portal: Status (index runs, servers, folders) and Search (the best passage of each file). Only that user may open it. An install from before needs a domain and a path set first: see [before upgrading](doc/PRE_UPGRADE.d/0.13.0~ynh1.md).

## 0.12.1~ynh1

- An index run an agent asks for with `run_index` no longer pings healthchecks.io, so the check stays an alert on the scheduled runs alone. A request waiting when the timer fires is answered by the full run.
- While a run goes on, the panel's next run says "after this run", no longer that the schedule is never.

## 0.12.0~ynh1

- `move_file`: an agent moves or renames a file or a folder within a writable folder, never replacing anything.
- A write Nextcloud refuses because the file is locked says it is, or was, open in Nextcloud's Text editor.
- The app's services reach on this server only what they use: the embedding and reranking models, Nextcloud and Whoopy, no longer its databases or other services. The Whoopy address for desktop embedding must be `http://127.0.0.1:<port>`.
- How to add the server to Whoopy since its 0.2.0~ynh5: on Whoopy's status page, as the server `docs`; a new token is given to it in Whoopy's admin page.

## 0.11.0~ynh2

- `sudo cheruby-docs-mcp token` shows the address and the tokens, and `token --new` makes a new Whoopy token, as the panel's "New token" does. Every other command of the binary answers as `sudo cheruby-docs-mcp <command>`.
- The panel's "Whoopy token" section is now "MCP token", and its setting `mcp_token`, as in the other MCP apps here. The upgrade keeps the token as it is: Whoopy needs nothing.
- Each tool, its arguments and its limits in [MCP tools](doc/mcp.md).

## 0.11.0

- The tool server, the indexer and their commands are one binary built for the Pi 5, in place of the Python and its virtual environment. The index is kept.
- `run_index`: an agent asks for an index run of some files, such as one it just wrote, or of all the folders, so search finds what changed. Refused in the quiet hours, past 50 files and while 20 requests wait.
- The binary answers on the shell as root: `ls`, `find`, `read`, `search`, `run-index`, `status` and `check-desktop`.

## 0.10.1

- A redrawn logo.

## 0.10.0

- "Run now on the desktop" in "Index runs": the passages are turned into vectors by the desktop's GPU through Whoopy, so a first index of many files no longer takes hours. Set the model as Whoopy names it and a Whoopy key under "Desktop embedding". The run checks first that the desktop's vectors agree with the server's, and goes on on the server when they do not or when Whoopy stops answering. The schedule never uses it.
- An index run stops, instead of marking each file failed, when the embedding server refuses it or holds it back.

## 0.9.1

- Qwen3-Embedding can be picked in the panel: its two choices were unreadable and the panel refused them.

## 0.9.0

- "Embedding model" in the config panel: EmbeddingGemma 300M (the default, as before), jina-embeddings-v5-text-nano (non-commercial use only) or Qwen3-Embedding 0.6B, each in Q8 or a smaller Q4. Only the chosen model is downloaded. A change makes the next index run read every file again; until then search matches keywords only, and says so.
- Install, upgrade and restore no longer compile llama-server on the server, about 8 minutes each: they download a release built for the Raspberry Pi 5 and pinned by its checksum. The build tools are no longer installed.
- The first index run after the upgrade reads every file again, about half an hour, since the new llama-server counts as a new model; search matches keywords only until its first results are published.

## 0.8.0

- Search passages follow the document's headings: a short note or section stays whole, a longer one is cut at its subheadings, and each passage starts with its file name and headings. Passages are about half as long, so an agent reads less beside the answer. The first index run after the upgrade reads every file again, about half an hour, while search answers from what it has done so far.
- `read_file` shows the headings of Word and OpenDocument files as Markdown `#` lines.

## 0.7.0

- Search orders its results by how well they answer the question, with a reranking model run on the server, and shows the best passage of each file. On the author's test queries, 9 of 9 expected documents come in the top 5, against 7 before, in about 4 seconds a search. The model, jina-reranker-v2-base-multilingual, is for non-commercial use only.
- When the reranking model does not answer, search still answers, in a rougher order, and says so.

## 0.6.2

- "Whoopy access" is picked from the open folders instead of typed, so a mistyped label can no longer be saved.

## 0.6.1

- An index run stuck past 6 hours now fails, so the timer starts the next one.
- A Nextcloud app password refused once is not tried again until a new one is saved: each failed login made Nextcloud hold back logins from every app on the server. When it does, the panel says so and how to clear it.
- The panel checks again whether the app can read each folder on disk; it said "could not be checked" for all of them.

## 0.6.0

- "Writable folders" per source, and a `write_file` tool that creates or replaces a text file in them, never deletes. Never through a link, never a file with other hard links, never the last 2 GB of the disk; the panel says what a folder's group cannot change.
- A `create_folder` tool beside `write_file`.
- The index runs every hour, every 6 hours, every night, never, or on a custom cron schedule, chosen in the panel, never in the quiet hours set there; a run still going stops when they begin and the next one carries on.
- "Index runs" in the panel: how the last run ended, what the index holds, the next run, and "Run now" and "Rebuild" buttons.

## 0.5.0

- A Nextcloud source: folders of a Nextcloud account, shares from others included, read through Nextcloud with an app password made there and pasted in the panel. The account can be another login than the user, given read-only shares.
- The panel shows, per Nextcloud folder, whether Nextcloud lets the app read it.
- The index lists again only the Nextcloud folders that changed; when Nextcloud does not answer, its passages stay and the run is reported as failed.

## 0.4.0

- One user, chosen at install; their folders come from sources chosen in the config panel: Syncthing folders ticked from Syncthing's list, and local folders.
- "Only these files" and "Leave out" patterns per source; what they leave out cannot be listed, found, read or searched, even through a link.
- The panel shows, per folder, the files the app cannot read, and whether Syncthing ignores permissions.
- A folder just added no longer stops search until the next index run.
- The upgrade turns the old `folders` setting into the new sources.

## 0.1.0 (unreleased)

- Lists, finds by name and reads each user's folders over MCP: text, Markdown, PDF by page, Word, OpenDocument and images.
- Each user's token reaches that user's folders only; nothing is ever written.
