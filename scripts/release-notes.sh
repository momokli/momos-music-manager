#!/bin/bash
# Generate the "Downloads + SHA256SUMS" section that is prepended to the
# auto-generated changelog of a tag release (M2 DoD: platform matrix + SHA256
# must be part of the tag release notes).
#
# Mirrors the artifact name schema in scripts/publish-assets.sh (single source
# of truth for names) and embeds the aggregate SHA256SUMS produced by
# `publish-assets.sh stage-tag`.
#
# Usage: release-notes.sh <version> <dist-dir>
#
# Writes markdown to stdout. Safe for the PR publish-gate dry run: no network,
# no release, no side effects beyond reading <dist-dir>/SHA256SUMS.
set -euo pipefail

version="${1:?usage: release-notes.sh <version> <dist-dir>}"
dist="${2:?usage: release-notes.sh <version> <dist-dir>}"
sha_file="${dist}/SHA256SUMS"
[[ -f "${sha_file}" ]] || {
    echo "SHA256SUMS not found in ${dist} (run publish-assets.sh stage-tag first)" >&2
    exit 1
}

# Static parts use a quoted heredoc so markdown backticks are literal; the
# version is substituted with printf, and the aggregate checksum block is
# emitted verbatim from the staged file.
cat <<'EOF'
## Downloads

| Platform | Architecture | Artifact |
|----------|--------------|----------|
EOF
printf '| macOS | Universal (Apple Silicon + Intel) | `momos-music-manager-%s-macos-universal.dmg` |\n' "${version}"
printf '| Windows | x64 | `momos-music-manager-%s-windows-x64.zip` |\n' "${version}"
printf '| Windows | ARM64 | `momos-music-manager-%s-windows-arm64.zip` |\n' "${version}"
printf '| Linux | x64 | `momos-music-manager-%s-linux-x64.tar.gz` |\n' "${version}"
printf '| Linux | ARM64 | `momos-music-manager-%s-linux-arm64.tar.gz` |\n' "${version}"

cat <<'EOF'

Each artifact has a matching `.sha256` file; the aggregate manifest below
(`SHA256SUMS`) is Ed25519-signed (`SHA256SUMS.minisig`).

## SHA256SUMS

```text
EOF
cat "${sha_file}"
cat <<'EOF'
```

## Verify

Compare the checksum of your download against the `SHA256SUMS` entry above:

- **macOS:** `shasum -a 256 <file>`
- **Linux:** `sha256sum <file>`
- **Windows:** `certutil -hashfile <file> SHA256`

The signed manifest can be verified with
[minisign](https://jedisct1.github.io/minisign/):

```
minisign -V -p <public-key> -m SHA256SUMS
```
EOF
