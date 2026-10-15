# kellner

**kellner** is the local, cross-platform **companion** of the music stack. It runs
on each Mac / PC and keeps the local machine in sync with the always-on servers
[`music-api`](../music-api/) (downloads, object store, BPM/key authority) and
[`music-hub`](../music-hub/) (multi-user web UI + DB).

> **Status: 0.1.0 shipped — all plan phases P0–P6 implemented.** `kellner pair`
> registers this device with the hub; `traktor-upload`, `backpack`, `backup` and
> `downloads` sync; `kellner run` is the background loop the OS service executes,
> and `kellner service install|uninstall|status` writes the launchd / systemd
> unit (Task Scheduler hint on Windows). An opt-in tray / menu-bar widget
> (`--features tray`) shows `feierabend` / `re-auth`. See [`CHANGELOG.md`](CHANGELOG.md).

## What it does

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

## Usage

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

# Background loop (what the OS service runs) — power-aware on battery
cargo run -- run
cargo run -- run --once                       # single pass, then exit
cargo run -- run --no-power-awareness         # fixed interval

# Install / inspect the OS service unit
cargo run -- service install                  # writes the unit + prints the enable command
cargo run -- service install --print          # print the unit text, write nothing
cargo run -- service status
cargo run -- service uninstall
```

The tray / menu-bar widget (right-click: `feierabend`, `re-auth`) is behind the
**off-by-default** `tray` cargo feature, so headless hosts and the CI build keep
working without GTK / appindicator system libraries:

```bash
cargo run --features tray -- run
```

Config lives in `~/.config/kellner/config.toml` (hub-host override + device
identity, file mode `0600`). Override the path with `KELLNER_CONFIG`; override
the hub with `KELLNER_HUB_URL`.

## Service units (macOS / Linux / Windows)

`kellner service install` writes:

- **macOS** — `~/Library/LaunchAgents/de.zukkafabrik.kellner.plist`
  (battery-friendly: `ProcessType=Background`, `Nice=10`, `LowPriorityIO`,
  `RunAtLoad`, `KeepAlive`), then prints `launchctl load -w …`.
- **Linux** — `$XDG_CONFIG_HOME/systemd/user/kellner.service` (else
  `~/.config/…`), then prints `systemctl --user enable --now kellner.service`.
- **Windows** — no file is written; a `schtasks /Create …` hint is printed.

`kellner run` scales its poll interval by power source
(`power::interval_for`): AC×1, AC+idle×2, battery×3, battery+idle×6, with a 30 s
floor — so a laptop on battery polls noticeably less often. The sleep itself is
interruptible: `Ctrl-C` (SIGINT), `SIGTERM` (how `systemctl --user stop kellner`
stops a unit) and the tray's `feierabend` all stop the loop within ~500 ms
instead of waiting out the current interval.

### Tray screenshots

The tray / menu-bar widget is a **native, non-web** UI (`tray.rs`) that needs a
running GTK/appindicator (Linux) or macOS window server and a paired pointer,
so it cannot be rendered in headless CI. It is therefore **exempt from the
screenshot requirement** for UI changes; its observable contract — exactly the
menu entries `feierabend` and `re-auth` — is pinned by the unit test
`tray::tests::menu_has_exactly_feierabend_and_reauth` instead.

## Install & smoke test

Build a release binary (no tray feature) and install it, or verify an existing
build offline:

```bash
./scripts/install.sh                  # -> ${PREFIX:-$HOME/.local}/bin/kellner
PREFIX=/usr/local ./scripts/install.sh --service   # install + register the unit
./scripts/smoke.sh                    # offline CLI check (no network, no pairing)
```

`scripts/smoke.sh` asserts `--version`, `--help` (all subcommands) and
`service install --print --dry-run` (prints a unit, writes nothing); a paired
pass is opt-in via `KELLNER_SMOKE_RUN=1`. Reference unit templates live under
[`deploy/`](deploy/).

## Tests

The end-to-end loop (upload → export → order → fetch → tag → backup) is driven
against one in-process mock hub + music-api in [`tests/e2e.rs`](tests/e2e.rs);
the per-command flows live in the sibling `tests/*.rs`. Run them with
`cargo test --locked`.

## Release

The 0.1.0 slice (P0–P6) is shipped. See [`CHANGELOG.md`](CHANGELOG.md),
[`AGENT.md`](AGENT.md) and [`../docs/ROADMAP.md`](../docs/ROADMAP.md).
