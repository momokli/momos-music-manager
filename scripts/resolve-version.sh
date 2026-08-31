#!/bin/bash
# Resolve the effective build version — single source of truth for CI and
# local packaging (see docs/versioning.md for the full schema).
#
#   GITHUB_REF == refs/tags/v1.2.0  -> 1.2.0                (tag is the truth)
#   GITHUB_REF == refs/heads/main   -> <cargo>-dev+<sha8>   (rolling dev)
#   anything else / no GITHUB_REF   -> <cargo>-dev+<sha8>   (branches/PR/local)
#
# dev formula: <Cargo.toml version>-dev+<git rev-parse --short=8 HEAD>
# SemVer: `-dev+<sha8>` = pre-release `dev` + build metadata `<sha8>`.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
    echo "${GITHUB_REF#refs/tags/v}"
    exit 0
fi

CARGO_VERSION=$(cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
SHA8=$(git rev-parse --short=8 HEAD)
echo "${CARGO_VERSION}-dev+${SHA8}"
