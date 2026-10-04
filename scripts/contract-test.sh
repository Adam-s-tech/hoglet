#!/usr/bin/env bash
# SDK contract tests: real PostHog SDKs at `latest` against a real Hoglet
# binary (claims.md claim 1).
#
#   scripts/contract-test.sh [node|browser|python|all]
#
# Env:
#   HOGLET_BIN                 use this binary instead of `cargo build`
#   CONTRACT_PLAYWRIGHT_DEPS=1 also install Chromium's system deps (CI)
#   CONTRACT_KEEP_DATA=1       keep Hoglet data dirs and logs after a run
#
# Each suite prints PASS/FAIL per assertion and writes
# contract-tests/results/<suite>.json. Exit status is non-zero if any
# assertion in any selected suite failed.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TESTS="$ROOT/contract-tests"
WHICH="${1:-all}"

case "$WHICH" in
  node | browser | python) SUITES=("$WHICH") ;;
  all) SUITES=(node browser python) ;;
  *)
    echo "usage: $0 [node|browser|python|all]" >&2
    exit 2
    ;;
esac

if [[ -z "${HOGLET_BIN:-}" ]]; then
  echo "== cargo build"
  (cd "$ROOT" && cargo build --quiet) || exit 1
  export HOGLET_BIN="$ROOT/target/debug/hoglet"
fi
# Suites run from contract-tests/; make a relative path absolute first.
HOGLET_BIN="$(cd "$(dirname "$HOGLET_BIN")" && pwd)/$(basename "$HOGLET_BIN")"
export HOGLET_BIN
echo "== hoglet binary: $HOGLET_BIN"

cd "$TESTS" || exit 1
# Always resolve SDKs to `latest` — never pinned, never a stale lockfile.
echo "== npm install (SDKs at latest)"
npm install --no-audit --no-fund --no-package-lock --loglevel=error \
  posthog-node@latest posthog-js@latest playwright@latest --no-save >/dev/null || exit 1

for suite in "${SUITES[@]}"; do
  if [[ "$suite" == browser ]]; then
    deps=()
    [[ "${CONTRACT_PLAYWRIGHT_DEPS:-}" == 1 ]] && deps=(--with-deps)
    node node_modules/playwright/cli.js install "${deps[@]}" chromium >/dev/null || exit 1
  fi
done

rm -f results/*.json
status=0
for suite in "${SUITES[@]}"; do
  node "$suite.test.mjs" || status=1
done

echo
echo "== summary"
for suite in "${SUITES[@]}"; do
  node -e '
    const fs = require("fs");
    const f = `results/${process.argv[1]}.json`;
    if (!fs.existsSync(f)) { console.log(`${process.argv[1]}: CRASHED (no results)`); process.exit(); }
    const r = JSON.parse(fs.readFileSync(f, "utf8"));
    const sdk = Object.entries(r.meta.sdk ?? {}).map(([k, v]) => `${k} ${v}`).join(", ");
    console.log(`${r.failed ? "FAIL" : "PASS"}  ${r.suite}: ${r.passed} passed, ${r.failed} failed  [${sdk}]`);
  ' "$suite"
done
exit $status
