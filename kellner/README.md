# kellner

**kellner** is the local, cross-platform **companion** of the music stack. It runs
on each Mac / PC and keeps the local machine in sync with the always-on servers
[`music-api`](../music-api/) (downloads, object store, BPM/key authority) and
[`music-hub`](../music-hub/) (multi-user web UI + DB).

> **Status: pairing implemented (#362).** `kellner pair` registers this device with
> the hub and stores the device token. The remaining subcommands (`backpack`,
> `backup`, `traktor-upload`, `downloads`) still print `not yet implemented`.

## What it will do

- **Backpack upkeep** — keep the local Backpack (the selection of playlists/tags
  the user cares about) in sync.
- **Library backup** — back up the local music library to `music-api`.
- **Traktor upload** — upload a Traktor `collection.nml` (playcount / rating /
  playlists).
- **Downloads** — report the state of downloads and the Backpack.

## Scope boundary

kellner talks **only to `music-api`** (and, where needed, the hub). It never talks
to Deezer / Spotify / SoundCloud / YouTube providers directly — `music-api` is the
single authority for downloads and metadata.

## Usage (skeleton)

```bash
cd kellner
cargo run -- --help
cargo run -- --version
# Pair this device: opens the approval URL; the user approves in the browser
# and presses Enter. The raw token is never printed.
cargo run -- pair
cargo run -- pair --no-open            # just print the approval URL
cargo run -- pair --print-only         # register + print, no binding/persist (CI)
cargo run -- backpack
cargo run -- backup
cargo run -- traktor-upload
cargo run -- downloads
```

Config lives in `~/.config/kellner/config.toml` (hub-host override + device
identity, file mode `0600`). Override the path with `KELLNER_CONFIG`; override
the hub with `KELLNER_HUB_URL`.

## Planned first features (kellner-0.1.0)

Pairing, Traktor upload, backups, downloads, Backpack upkeep, service/tray.
See [`AGENT.md`](AGENT.md) and [`../docs/ROADMAP.md`](../docs/ROADMAP.md).
