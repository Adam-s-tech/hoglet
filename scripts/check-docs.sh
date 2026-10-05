#!/usr/bin/env bash
# Docs check: every HOGLET_* variable named in the user docs or the README must
# exist in the code, so the docs cannot promise a setting that is not there.
#
#   scripts/check-docs.sh
#
# Server variables are looked up in src/main.rs. Installer-only variables
# (HOGLET_REPO, HOGLET_DOWNLOAD_URL, HOGLET_VERSION) are looked up in
# scripts/install.sh. Exit status is non-zero if any variable is unknown.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail=0
vars="$(grep -rhoE 'HOGLET_[A-Z][A-Z0-9_]*' docs/user README.md | sort -u)"
for var in $vars; do
    if grep -qw "$var" src/main.rs || grep -qw "$var" scripts/install.sh; then
        continue
    fi
    echo "docs mention $var, which is not in src/main.rs or scripts/install.sh" >&2
    fail=1
done

if [ "$fail" -eq 0 ]; then
    echo "docs ok: $(echo "$vars" | wc -l | tr -d ' ') HOGLET_* variables, all exist"
fi
exit "$fail"
