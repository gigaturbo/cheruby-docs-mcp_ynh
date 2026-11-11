# Changelog

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
