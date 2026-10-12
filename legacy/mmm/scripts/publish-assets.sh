#!/bin/bash
# Shared publish-asset logic for .github/workflows/build-all.yml.
#
# Single source of truth for everything the publish job derives from the
# build artifacts, so the real publish and the PR "publish gate" can never
# drift apart:
#
#   validate <dist-dir>
#       Assert that <dist-dir> contains EXACTLY the artifact set the publish
#       step expects for the current version (see resolve-version.sh), and
#       that every .sha256 records the matching hash + file name. Fails with
#       a precise diff. No side effects.
#
#   stage-main <dist-dir>
#       Everything the publish job does for refs/heads/main that is local:
#       stable-name copies (landing page), their checksums, the aggregate
#       SHA256SUMS and the upload manifest (.upload-manifest, one file per
#       line). No release/sign/upload — no network side effects.
#
#   stage-tag <dist-dir>
#       Same for tag publishes (refs/tags/v*): aggregate SHA256SUMS over the
#       versioned artifacts + upload manifest (no stable names).
#
# The workflow calls these with the same arguments in the real publish job
# and in the PR gate; only the gate never runs the gh/minisign steps.
#
# Artifact name schema (produced by the package jobs, consumed here):
#   momos-music-manager-<version>-<platform>.<ext>
#   momos-music-manager-<version>-<platform>.<ext>.sha256
# with <platform>.<ext> per build job:
#   macos-universal .dmg, windows-x64 .zip, windows-arm64 .zip,
#   linux-x64 .tar.gz, linux-arm64 .tar.gz
# <version> comes from scripts/resolve-version.sh (dev+sha8 on branches).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Upload order mirrors the historic explicit list in build-all.yml.
# "<platform> <extension>"
PLATFORM_SPECS=(
  "macos-universal dmg"
  "windows-x64 zip"
  "windows-arm64 zip"
  "linux-x64 tar.gz"
  "linux-arm64 tar.gz"
)

STABLE_NAMES=(
  "Momo-s-Music-Manager-latest.dmg"
  "momos-music-manager-latest-windows-x64.zip"
  "momos-music-manager-latest-windows-arm64.zip"
  "momos-music-manager-latest-linux-x64.tar.gz"
  "momos-music-manager-latest-linux-arm64.tar.gz"
)

die() {
  echo "::error::${*}" >&2
  exit 1
}

resolve_version() {
  bash "${SCRIPT_DIR}/resolve-version.sh"
}

# echo the 10 versioned file names (artifact + .sha256 per platform).
versioned_files() {
  local version="$1" spec ext
  for spec in "${PLATFORM_SPECS[@]}"; do
    ext="${spec#* }"
    echo "momos-music-manager-${version}-${spec%% *}.${ext}"
    echo "momos-music-manager-${version}-${spec%% *}.${ext}.sha256"
  done
}

# echo the 10 stable file names (artifact + .sha256 per platform).
stable_files() {
  local name
  for name in "${STABLE_NAMES[@]}"; do
    echo "${name}"
    echo "${name}.sha256"
  done
}

# Write the upload manifest (one file name per line, relative to dist) in
# the same order the publish job historically listed the files explicitly.
write_manifest() {
  local version="$1" stable="$2"
  {
    versioned_files "${version}"
    echo SHA256SUMS
    echo SHA256SUMS.minisig
    if [[ "${stable}" == main ]]; then
      stable_files
    fi
  } > .upload-manifest
}

check_versioned_present() {
  local dist="$1" version="$2" missing=0 f
  for f in $(versioned_files "${version}"); do
    if [[ ! -f "${dist}/${f}" ]]; then
      echo "::error::Missing expected artifact: ${f}" >&2
      missing=1
    fi
  done
  (( missing == 0 )) || exit 1
}

cmd_validate() {
  local dist="$1"
  local version
  version="$(resolve_version)"
  echo "Version: ${version}"

  [[ -d "${dist}" ]] || die "dist dir not found: ${dist}"

  # Strict set comparison: every expected file present, nothing unexpected
  # (an unexpected file means a build job deviated from the schema — exactly
  # the class of bug that broke the main publish for linux-arm64, which
  # packaged with the fallback Cargo version instead of the dev version).
  local -a expected actual missing unexpected
  mapfile -t expected <<< "$(versioned_files "${version}")"
  mapfile -t actual < <(ls -1A "${dist}")

  missing=()
  unexpected=()
  local f a
  for f in "${expected[@]}"; do
    if [[ ! -f "${dist}/${f}" ]]; then
      missing+=("${f}")
    fi
  done
  for a in "${actual[@]}"; do
    if [[ "${a}" == .upload-manifest || "${a}" == SHA256SUMS* ]]; then
      # Staging leftovers — only possible if validate runs after a stage.
      continue
    fi
    local found=0 e
    for e in "${expected[@]}"; do
      [[ "${a}" == "${e}" ]] && found=1 && break
    done
    (( found == 1 )) || unexpected+=("${a}")
  done

  if (( ${#missing[@]} > 0 )); then
    echo "::error::Artifact set incomplete (${#missing[@]} expected files missing):" >&2
    printf '  missing: %s\n' "${missing[@]}" >&2
  fi
  if (( ${#unexpected[@]} > 0 )); then
    echo "::error::Unexpected files in artifact set (${#unexpected[@]}) — a build job deviated from the schema:" >&2
    printf '  unexpected: %s\n' "${unexpected[@]}" >&2
  fi
  (( ${#missing[@]} == 0 && ${#unexpected[@]} == 0 )) || exit 1

  # Checksum files must reference the matching artifact name and hash.
  # (sha256sum -c is not used: Windows writes CRLF + a bare name.)
  local spec ext artifact sha recorded rec_name ok=0
  for spec in "${PLATFORM_SPECS[@]}"; do
    ext="${spec#* }"
    artifact="momos-music-manager-${version}-${spec%% *}.${ext}"
    sha="${artifact}.sha256"
    read -r recorded < <(head -n 1 "${dist}/${sha}")
    local recorded_hash recorded_name computed
    recorded_hash="$(awk '{print $1}' <<<"${recorded}")"
    recorded_name="$(awk '{$1=""; sub(/^[ ]+/,""); sub(/\r$/,""); print}' <<<"${recorded}")"
    computed="$(sha256sum "${dist}/${artifact}" | awk '{print $1}')"
    if [[ "${recorded_hash}" != "${computed}" ]]; then
      echo "::error::Checksum mismatch for ${artifact} (recorded ${recorded_hash}, computed ${computed})" >&2
      ok=1
    fi
    if [[ "${recorded_name}" != "${artifact}" ]]; then
      echo "::error::Checksum file ${sha} names '${recorded_name}', expected '${artifact}'" >&2
      ok=1
    fi
    echo "ok: ${artifact} (${computed})"
  done
  (( ok == 0 )) || exit 1

  echo "validate: artifact set complete and consistent (${#expected[@]} files) for version ${version}"
}

cmd_stage_main() {
  local dist="$1"
  local version
  version="$(resolve_version)"
  echo "Version: ${version}"
  check_versioned_present "${dist}" "${version}"
  cd "${dist}"

  # Stable names for the landing page download buttons (schema files, same
  # as the historic publish step). Fails with the classic
  # "cp: cannot stat ..." error if a build job produced a deviating name.
  cp "momos-music-manager-${version}-macos-universal.dmg" "Momo-s-Music-Manager-latest.dmg"
  cp "momos-music-manager-${version}-windows-x64.zip" "momos-music-manager-latest-windows-x64.zip"
  cp "momos-music-manager-${version}-windows-arm64.zip" "momos-music-manager-latest-windows-arm64.zip"
  cp "momos-music-manager-${version}-linux-x64.tar.gz" "momos-music-manager-latest-linux-x64.tar.gz"
  cp "momos-music-manager-${version}-linux-arm64.tar.gz" "momos-music-manager-latest-linux-arm64.tar.gz"

  # Matching checksums for the stable names (content-identical to the
  # versioned artifacts, but the file name inside the .sha256 matters).
  sha256sum "Momo-s-Music-Manager-latest.dmg" > "Momo-s-Music-Manager-latest.dmg.sha256"
  sha256sum momos-music-manager-latest-windows-x64.zip > momos-music-manager-latest-windows-x64.zip.sha256
  sha256sum momos-music-manager-latest-windows-arm64.zip > momos-music-manager-latest-windows-arm64.zip.sha256
  sha256sum momos-music-manager-latest-linux-x64.tar.gz > momos-music-manager-latest-linux-x64.tar.gz.sha256
  sha256sum momos-music-manager-latest-linux-arm64.tar.gz > momos-music-manager-latest-linux-arm64.tar.gz.sha256

  # Aggregate checksum file: versioned + stable entries (see build-all.yml —
  # the stable *.sha256 files are matched by the glob, the stable DMG is
  # added explicitly because its name starts with "Momo-").
  local -a sha_files=( momos-music-manager-*.sha256 )
  sha_files+=( "Momo-s-Music-Manager-latest.dmg.sha256" )
  cat "${sha_files[@]}" > SHA256SUMS
  echo "--- SHA256SUMS ---"
  cat SHA256SUMS

  # Upload manifest = versioned + stable + aggregate, in the historic upload
  # order. The publish step reads it instead of maintaining a second explicit
  # list; SHA256SUMS.minisig is created by the minisign step after this, so
  # the publish step re-checks every entry pre-upload.
  write_manifest "${version}" main
  echo "stage-main: wrote .upload-manifest with $(wc -l < .upload-manifest) entries"
  cat .upload-manifest
}

cmd_stage_tag() {
  local dist="$1"
  local version
  version="$(resolve_version)"
  echo "Version: ${version}"
  check_versioned_present "${dist}" "${version}"
  cd "${dist}"

  # Aggregate checksum file: versioned entries only on tag releases.
  local -a sha_files=( momos-music-manager-*.sha256 )
  cat "${sha_files[@]}" > SHA256SUMS
  echo "--- SHA256SUMS ---"
  cat SHA256SUMS

  write_manifest "${version}" tag
  echo "stage-tag: wrote .upload-manifest with $(wc -l < .upload-manifest) entries"
  cat .upload-manifest
}

main() {
  local cmd="${1:-}" dist="${2:-}"
  case "${cmd}" in
    validate)
      [[ -n "${dist}" ]] || die "usage: publish-assets.sh validate <dist-dir>"
      cmd_validate "${dist}"
      ;;
    stage-main)
      [[ -n "${dist}" ]] || die "usage: publish-assets.sh stage-main <dist-dir>"
      cmd_stage_main "${dist}"
      ;;
    stage-tag)
      [[ -n "${dist}" ]] || die "usage: publish-assets.sh stage-tag <dist-dir>"
      cmd_stage_tag "${dist}"
      ;;
    *)
      die "usage: publish-assets.sh <validate|stage-main|stage-tag> <dist-dir>"
      ;;
  esac
}

main "$@"
