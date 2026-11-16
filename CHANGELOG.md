# Changelog

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
