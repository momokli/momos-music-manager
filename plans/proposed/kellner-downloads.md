# Plan: kellner-downloads

**Status**: proposed
**Branch**: `feat/kellner-downloads`
**Ready for review**: no
**Depends on**: a kellner release tag (`kellner-v*`)
**Migration needed**: no

### Description

Offer the **kellner binary for every OS as a download in the hub** (the `/kellner`
page), in the current `hub.zukkafabrik.de` deployment: pick your platform, get the
file plus the checksum and the one-line install command.

Today that is impossible — not because the hub can't render a link, but because
**no binary exists anywhere**:

| Verified 2026-10-16           | State                                                                                                                               |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `momokli/momos-music-manager` | **public** → GitHub release assets need no auth                                                                                     |
| `.github/workflows/`          | `ci-pr`, `hub-ci`, `mmm-freeze`, `pages`, `pr-quality` — **no release/asset/cross-build workflow**, no matrix, no `upload-artifact` |
| kellner binaries              | none published; `scripts/install.sh` builds from source locally                                                                     |
| hub file serving              | none (`ServeDir`/`rust-embed` absent); only dynamic ZIP streams from music-api                                                      |

So the work is: **produce the artefacts**, then **surface them**.

### Milestones

**M1 — Publish the artefacts** — `.github/workflows/kellner-release.yml` (new), `kellner/Cargo.toml`

Triggered by the component tag `kellner-v*`. **Tray follows the platform, not a
preference:** `tray-icon` is native on macOS/Windows (no extra system libs) but pulls
GTK/appindicator on Linux, so the Linux artefacts stay headless — the same split
`scripts/install.sh` already implies.

| Runner                             | Target                       | Variant  | Artefact                                        |
| ---------------------------------- | ---------------------------- | -------- | ----------------------------------------------- |
| `macos-14`                         | `aarch64-apple-darwin`       | tray     | `kellner-<ver>-aarch64-apple-darwin.tar.gz`     |
| `macos-14` (cross)                 | `x86_64-apple-darwin`        | tray     | `kellner-<ver>-x86_64-apple-darwin.tar.gz`      |
| `windows-latest`                   | `x86_64-pc-windows-msvc`     | tray     | `kellner-<ver>-x86_64-pc-windows-msvc.zip`      |
| `ubuntu-latest`                    | `x86_64-unknown-linux-gnu`   | headless | `kellner-<ver>-x86_64-unknown-linux-gnu.tar.gz` |
| `ubuntu-24.04-arm` (or cross+qemu) | `aarch64-unknown-linux-gnu`  | headless | `…-aarch64-unknown-linux-gnu.tar.gz`            |
| `ubuntu-latest` + `cargo-zigbuild` | `x86_64-unknown-linux-musl`  | headless | `…-x86_64-unknown-linux-musl.tar.gz`            |
| `ubuntu-latest` + `cargo-zigbuild` | `aarch64-unknown-linux-musl` | headless | `…-aarch64-unknown-linux-musl.tar.gz`           |

Each leg: `cargo build --release --locked --target <t>` → **run** `<binary> --version`
(native legs; aarch64-gnu natively on the arm runner — the reason to prefer it over
qemu) → upload as a **release asset** on the tag, plus one `SHA256SUMS` for all of
them. A `cargo test --locked` gate runs before publishing.

**M1b — Make them small** — `kellner/Cargo.toml`

The crate has **no `[profile.release]`** today, so the artefacts are unstripped and
un-LTO'd. Add one (`lto`, `codegen-units = 1`, `strip = true`, `opt-level = "z"`,
optionally `panic = "abort"`) and **measure**: the plan's table must show the size per
artefact so „klein" is a number, not a feeling.

⚠️ `panic = "abort"` is a behaviour change (a panic becomes an abort instead of an
unwind). It is reasonable for a supervised service (systemd `Restart=on-failure`,
launchd `KeepAlive`) but `cargo test` **cannot** catch it — it runs the test profile.
Verify it by letting the installed service hit a failure once, or leave it out.

**M2 — Surface them in the hub** — `music-hub/src/device_settings.rs`, `templates/kellner.html`

A „Kellner herunterladen" section on the **existing `/kellner` page** (it already
exists for device settings — no new route, no new nav entry):

- one row per target: OS/arch label, **tray or headless**, version, size, `SHA256`,
  direct link — with the default recommendation marked (mac/win tray, linux headless);
- the link is a **stable** URL that always resolves to the current release
  (`/releases/latest/download/<name>`), so the page needs no rebuild per release;
- below it, per OS, the **autostart** command — „auto-start und da" is the whole point of
  the service, so it is not a footnote: `kellner service install` (launchd / systemd
  --user) and the Task-Scheduler line for Windows;

**M1 — testability**: the workflow gets `workflow_dispatch` **as well as** the tag
trigger, so the whole matrix can be proven on a branch before any release is cut —
the workflows themselves are otherwise only verifiable by tagging.

The version shown comes from a setting, not from a live GitHub call (the hub must
not depend on GitHub being reachable to render a page): `settings::SETTINGS` gets
`kellner_download_version` (default `0.1.0`), editable at `/admin`. A stale value
only mislabels a link — the link itself stays correct.

**M3 — Make the downloads trustworthy** — `templates/kellner.html`

- Show `SHA256` and the matching `sha256sum`/`shasum -a 256` command per platform.
- macOS/Windows caveat, stated in the UI and not hidden: the binaries are **unsigned**,
  so Gatekeeper/SmartScreen will complain. Either document the explicit bypass
  (`xattr -d com.apple.quarantine ~/.local/bin/kellner`) or make signing a follow-up
  (needs a paid Apple Developer account).

**M4 — Verify** — no code

On each OS: download via the hub link, run `<file> --version`, run
`<file> service install --print`, and compare the checksum with `SHA256SUMS`.

### Acceptance Criteria

- [ ] `kellner-v*` tag push builds **all seven** targets/variants in CI and attaches them plus `SHA256SUMS`
- [ ] Each native leg runs `<file> --version` in CI (compiling is not enough)
- [ ] The release table shows a **measured size** per artefact, and the profile change is reflected there
- [ ] `/kellner` lists every target with variant, version, size, checksum and a working link
- [ ] `workflow_dispatch` runs the full matrix green **before** any tag exists
- [ ] The links use `/releases/latest/download/...` so no hub redeploy is needed per release
- [ ] The page renders with **no** network access from the hub to GitHub (version from a setting)
- [ ] The unsigned-binary caveat is visible in the UI, with the bypass command
- [ ] `cargo test` (music-hub) stays green; no migration

### Measured (CI, 2026-10-16, PR #474)

All seven legs green. Raw = the binary the leg built, packed = the artefact a user
downloads. The macOS headless reference is what `scripts/install.sh` builds locally:
**7,504,128 → 3,687,056 bytes (−51 %)** after the profile change.

| Leg                 | Raw       | Packed    |
| ------------------- | --------- | --------- |
| macos-arm64-tray    | 3,959,584 | 4,027,269 |
| macos-x86_64-tray   | 4,190,920 | 4,196,631 |
| windows-x86_64-tray | 4,878,336 | 4,258,202 |
| linux-x86_64-gnu    | 4,201,240 | 4,015,402 |
| linux-aarch64-gnu   | 3,742,280 | 3,800,066 |
| linux-x86_64-musl   | 4,336,208 | 4,179,172 |
| linux-aarch64-musl  | 3,717,056 | 3,915,819 |

Two legs are deliberately `smoke: skip` (the binary cannot be executed on the build
host). Both were verified **by hand** anyway: the macos-x86_64 artefact runs on an
arm64 Mac under Rosetta, and the aarch64-musl artefact runs in an arm64 Alpine
container — which is also how the cross-leg version bug was caught (`0.1.0
(unknown)`: `cross` builds in a container with no `.git` and no `KELLNER_VERSION`;
fixed by injecting both through cargo's `[env]` table).

**Not proven yet:** the `release` job (tag → attach assets + `SHA256SUMS`). It is
gated on a real `kellner-v*` tag, so every PR run skips it. Proving it means cutting
a tag.

### Decisions (settled 2026-10-16)

1. **Target set** — macOS arm64 + x86_64, Windows x86_64, Linux x86_64 **and arm64,
   gnu and musl**. arm64 and musl are explicit requirements, not nice-to-haves.
2. **Variants** — **tray on macOS and Windows** (native, no extra system libs),
   **headless on Linux** (GTK/appindicator is the price of a tray there).
   `tray-icon` is the only dependency either way and `tray.rs` has no `target_os`
   gates, so a Windows tray needs no new code.
3. **macOS signing** — none for 0.1.x: unsigned artefacts plus a documented Gatekeeper
   bypass. Signing/notarization is a later step (Apple Developer account).
4. **Hosting** — the hub **links** to GitHub release assets (repo is public, no auth,
   CDN-backed); mirroring into music-api's object store only if GitHub should stop
   being a dependency.

### Still open before M1

- **musl toolchain**: `cargo-zigbuild` (zig as linker, no Docker) vs. `cross`
  (Docker, also gives qemu for aarch64). Recommendation: zigbuild for both musl
  targets; `ubuntu-24.04-arm` for native aarch64-gnu so its smoke test really runs.
- **Linux tray later?** A `-tray` Linux artefact is possible for anyone with GTK;
  not in 0.1.x.

### Non-goals

- An auto-updater in kellner (the legacy MMM had one; kellner does not, yet).
- Serving the binaries **from** the hub itself (no file-serving path exists; embedding
  multi-MB artefacts into the hub binary via `rust-embed` is the wrong trade).
- A Windows installer (`msi`/`exe` with a Start-menu entry) — a zip plus the
  Task-Scheduler hint stays.
- Changing what `scripts/install.sh` does (source builds keep working as they are).
