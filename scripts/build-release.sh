#!/usr/bin/env bash
# Reproducible release build for one platform.
#
#   scripts/build-release.sh <linux-amd64|linux-arm64|darwin-arm64|darwin-amd64>
#
# Output: dist/hoglet-<version>-<os>-<arch>.tar.gz and a .sha256 next to it.
#
# Linux targets are fully static musl binaries (DuckDB's C++ and libc++ linked
# in) built with cargo-zigbuild, so one binary runs on any Linux distro and in
# a FROM scratch image. macOS targets are native builds and must run on macOS.
#
# Requirements:
#   linux-*:  rustup target for *-unknown-linux-musl, zig >= 0.13, cargo-zigbuild
#   darwin-*: Xcode command line tools, rustup target for *-apple-darwin
#
# Env:
#   HOGLET_VERSION     override the version string (default: Cargo.toml version)
#   SOURCE_DATE_EPOCH  timestamp baked into the tarball (default: HEAD commit time)
set -euo pipefail

usage() {
    echo "usage: $0 <linux-amd64|linux-arm64|darwin-arm64|darwin-amd64> [--smoke]" >&2
    echo "       $0 --smoke-only <path-to-hoglet-binary>" >&2
    exit 2
}

# Boot a binary on a scratch data dir and prove the install claim end to end:
# /ready, first-run setup returns a project token, POST /e/ accepts an event,
# the event is queryable through DuckDB, nothing was downloaded at runtime, and
# SIGINT shuts down cleanly. Prints cold-start time to /ready.
smoke() {
    smoke_bin="$1"
    smoke_runner="${SMOKE_RUNNER:-}" # e.g. qemu-aarch64-static for cross binaries
    smoke_data="$(mktemp -d)"
    smoke_port="${SMOKE_PORT:-18765}"
    smoke_url="http://127.0.0.1:$smoke_port"
    start_ns="$(date +%s%N 2>/dev/null || echo 0)"
    # Private HOME: catches DuckDB downloading extensions into ~/.duckdb.
    mkdir -p "$smoke_data/home"
    HOME="$smoke_data/home" HOGLET_ADDR="127.0.0.1:$smoke_port" HOGLET_DATA="$smoke_data/data" \
        $smoke_runner "$smoke_bin" > "$smoke_data.log" 2>&1 &
    smoke_pid=$!
    ready=0
    for _ in $(seq 1 600); do
        if curl -fsS -o /dev/null "$smoke_url/ready" 2>/dev/null; then ready=1; break; fi
        kill -0 "$smoke_pid" 2>/dev/null || break
        sleep 0.05
    done
    end_ns="$(date +%s%N 2>/dev/null || echo 0)"
    fail() {
        echo "smoke: FAIL: $*" >&2
        cat "$smoke_data.log" >&2
        kill "$smoke_pid" 2>/dev/null || true
        rm -rf "$smoke_data" "$smoke_data.log" "$smoke_data.jar"
        exit 1
    }
    [ "$ready" -eq 1 ] || fail "/ready never returned 200"
    case "$start_ns$end_ns" in
        *N*) echo "smoke: ready" ;;
        *) echo "smoke: ready in $(( (end_ns - start_ns) / 1000000 )) ms" ;;
    esac

    setup="$(curl -fsS -c "$smoke_data.jar" -X POST "$smoke_url/api/auth/setup" -H 'content-type: application/json' \
        -d '{"email":"smoke@example.com","password":"smoke-password-1","organization_name":"Smoke","project_name":"Smoke"}')" \
        || fail "POST /api/auth/setup"
    token="$(printf '%s' "$setup" | grep -o 'phc_[0-9a-f]\{32\}' | head -n 1)"
    [ -n "$token" ] || fail "setup response has no project token: $setup"

    code="$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$smoke_url/e/" -H 'content-type: application/json' \
        -d "{\"api_key\":\"$token\",\"event\":\"smoke_test\",\"distinct_id\":\"smoke-user\",\"properties\":{\"\$lib\":\"smoke\"}}")"
    [ "$code" = 200 ] || fail "POST /e/ returned $code"
    code="$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$smoke_url/e/" -H 'content-type: application/json' \
        -d '{"api_key":"phc_wrong","event":"x","distinct_id":"y"}')"
    case "$code" in 4??) ;; *) fail "unknown token returned $code, want 4xx" ;; esac
    echo "smoke: setup ok, capture 200, bad token $code"

    # The query lane: the captured event must come back out through DuckDB.
    # This is what proves DuckDB's parquet/json support is compiled in rather
    # than fetched at runtime (a static binary cannot load extensions).
    project="$(printf '%s' "$setup" | sed -n 's/.*"projects":\[{"id":"\([0-9a-f-]*\)".*/\1/p')"
    [ -n "$project" ] || fail "setup response has no project id: $setup"
    year="$(date -u +%Y)"
    trends="{\"query\":{\"kind\":\"Trends\",\"series\":[{\"event\":{\"type\":\"name\",\"value\":\"smoke_test\"},\"math\":{\"type\":\"total\"}}],\"filters\":{\"op\":\"AND\",\"values\":[]},\"range\":{\"from\":\"$((year - 1))-01-01T00:00:00Z\",\"to\":\"$((year + 1))-12-31T00:00:00Z\"},\"interval\":\"Month\"}}"
    counted=0
    for _ in $(seq 1 100); do
        result="$(curl -sS -b "$smoke_data.jar" -X POST "$smoke_url/api/projects/$project/query" \
            -H 'content-type: application/json' -d "$trends")" || fail "query request"
        case "$result" in
            *'"count":1'*) counted=1; break ;;
            *'"results"'*) sleep 0.1 ;; # not yet published to the lake
            *) fail "query failed: $result" ;;
        esac
    done
    [ "$counted" -eq 1 ] || fail "captured event never became queryable: $result"
    echo "smoke: query lane returned the captured event"

    kill -INT "$smoke_pid"
    for _ in $(seq 1 200); do kill -0 "$smoke_pid" 2>/dev/null || break; sleep 0.05; done
    if kill -0 "$smoke_pid" 2>/dev/null; then fail "did not exit within 10s of SIGINT"; fi
    wait "$smoke_pid" || fail "exited non-zero after SIGINT"
    echo "smoke: clean shutdown"
    if [ -d "$smoke_data/home/.duckdb" ]; then
        find "$smoke_data/home/.duckdb" -type f >&2
        fail "DuckDB fetched extensions at runtime; compile them in (duckdb features)"
    fi
    rm -rf "$smoke_data" "$smoke_data.log" "$smoke_data.jar"
}

if [ "${1:-}" = --smoke-only ]; then
    [ $# -eq 2 ] || usage
    smoke "$2"
    exit 0
fi

[ $# -ge 1 ] && [ $# -le 2 ] || usage
platform="$1"
run_smoke=0
if [ $# -eq 2 ]; then
    [ "$2" = --smoke ] || usage
    run_smoke=1
fi

case "$platform" in
    linux-amd64) triple=x86_64-unknown-linux-musl; builder=zigbuild ;;
    linux-arm64) triple=aarch64-unknown-linux-musl; builder=zigbuild ;;
    darwin-arm64) triple=aarch64-apple-darwin; builder=build ;;
    darwin-amd64) triple=x86_64-apple-darwin; builder=build ;;
    *) usage ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="${HOGLET_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}"
version="${version#v}"
[ -n "$version" ] || { echo "error: could not read version from Cargo.toml" >&2; exit 1; }

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH="$(git log -1 --format=%ct 2>/dev/null || echo 0)"
fi
export SOURCE_DATE_EPOCH

# Reproducibility: strip machine-specific paths out of panic messages and
# debug sections so two builds of one commit produce identical binaries.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$root=/hoglet --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$rustup_home=/rustup"
# Same for the bundled C/C++ (DuckDB, SQLite).
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$root=/hoglet -ffile-prefix-map=$cargo_home=/cargo"
export CXXFLAGS="${CXXFLAGS:-} -ffile-prefix-map=$root=/hoglet -ffile-prefix-map=$cargo_home=/cargo"

echo "building hoglet $version for $platform ($triple)"
if [ "$builder" = zigbuild ]; then
    command -v zig >/dev/null || { echo "error: zig not found (https://ziglang.org/download/)" >&2; exit 1; }
    command -v cargo-zigbuild >/dev/null || { echo "error: cargo-zigbuild not found (cargo install cargo-zigbuild)" >&2; exit 1; }
    # RUSTFLAGS replaces .cargo/config.toml rustflags, so restate crt-static.
    export RUSTFLAGS="$RUSTFLAGS -C target-feature=+crt-static"
    cargo zigbuild --release --locked --target "$triple"
else
    [ "$(uname -s)" = Darwin ] || { echo "error: $platform must be built on macOS" >&2; exit 1; }
    cargo build --release --locked --target "$triple"
fi

bin="target/$triple/release/hoglet"
[ -x "$bin" ] || { echo "error: $bin was not produced" >&2; exit 1; }

if [ "${platform%%-*}" = linux ]; then
    # The whole point of the linux build: no runtime dependencies at all.
    desc="$(file -b "$bin")"
    case "$desc" in
        *"statically linked"*|*"static-pie linked"*) ;;
        *) echo "error: $bin is not static: $desc" >&2; exit 1 ;;
    esac
    if command -v readelf >/dev/null && readelf -d "$bin" 2>/dev/null | grep -q NEEDED; then
        echo "error: $bin has dynamic NEEDED entries:" >&2
        readelf -d "$bin" | grep NEEDED >&2
        exit 1
    fi
fi

if [ "$run_smoke" -eq 1 ]; then
    smoke "$bin"
fi

name="hoglet-$version-$platform"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name"
cp "$bin" "$stage/$name/hoglet"
chmod 0755 "$stage/$name/hoglet"
cp LICENSE "$stage/$name/" 2>/dev/null || true
cp README.md "$stage/$name/" 2>/dev/null || true
cp -R deploy "$stage/$name/deploy"

mkdir -p dist
archive="dist/$name.tar.gz"

# Deterministic tarball: fixed order, fixed mtime, no owner names, no gzip timestamp.
tar_bin=tar
if command -v gtar >/dev/null; then tar_bin=gtar; fi
if "$tar_bin" --version 2>/dev/null | grep -q 'GNU tar'; then
    "$tar_bin" --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
        --format=gnu -C "$stage" -cf - "$name" | gzip -n -9 > "$archive"
else
    # bsdtar (stock macOS): normalise mtimes first, ownership flags differ.
    stamp="$(date -u -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)"
    find "$stage/$name" -exec env TZ=UTC0 touch -h -t "$stamp" {} +
    (cd "$stage" && find "$name" -print | LC_ALL=C sort | \
        tar --uid 0 --gid 0 --uname '' --gname '' -n -cf - -T -) | gzip -n -9 > "$archive"
fi

if command -v sha256sum >/dev/null; then
    (cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
else
    (cd dist && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
fi

size_bytes="$(wc -c < "$bin" | tr -d ' ')"
echo "binary:  $bin ($size_bytes bytes)"
echo "archive: $archive ($(wc -c < "$archive" | tr -d ' ') bytes)"
cat "dist/$name.tar.gz.sha256"
