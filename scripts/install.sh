#!/bin/sh
# Hoglet installer.
#
#   curl -fsSL <url>/install.sh | sh
#   curl -fsSL <url>/install.sh | sh -s -- --systemd
#
# Downloads the release tarball for this OS/arch from GitHub Releases, checks
# its sha256, and installs the `hoglet` binary.
#
# Options:
#   --version <v>   install a specific version (default: latest release)
#   --prefix <dir>  install directory (default: /usr/local/bin if writable or
#                   root, else ~/.local/bin)
#   --systemd       Linux only, needs root: create the `hoglet` system user,
#                   install /etc/systemd/system/hoglet.service, enable + start it
#   -h, --help      this text
#
# Env:
#   HOGLET_REPO          owner/name on GitHub (default: debpalash/hoglet)
#   HOGLET_DOWNLOAD_URL  base URL serving <tag>/<asset>, overrides GitHub
#                        (e.g. a mirror or internal artifact server)
#   GITHUB_TOKEN         token for a private repository's release assets
#   HOGLET_VERSION       same as --version
set -eu

HOGLET_REPO="${HOGLET_REPO:-debpalash/hoglet}"
version="${HOGLET_VERSION:-}"
prefix=""
systemd=0

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; version="$2"; shift 2 ;;
        --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix="$2"; shift 2 ;;
        --systemd) systemd=1; shift ;;
        -h|--help) sed -n '2,25p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=darwin ;;
    *) die "unsupported OS: $(uname -s) (Linux and macOS only)" ;;
esac
case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) die "unsupported architecture: $(uname -m) (amd64 and arm64 only)" ;;
esac
[ "$systemd" -eq 0 ] || [ "$os" = linux ] || die "--systemd is Linux only"
uid="$(id -u)"
[ "$systemd" -eq 0 ] || [ "$uid" -eq 0 ] || die "--systemd needs root (run with sudo)"
[ "$systemd" -eq 0 ] || command -v systemctl >/dev/null 2>&1 || die "--systemd: systemctl not found"
[ "$systemd" -eq 0 ] || [ -z "$prefix" ] || [ "$prefix" = /usr/local/bin ] || die "--systemd installs to /usr/local/bin; drop --prefix"

if command -v curl >/dev/null 2>&1; then
    # fetch <url> <out>: never sends credentials (mirrors, public assets).
    fetch() { curl -fsSL -o "$2" "$1"; }
    # fetch_gh <api-asset-url> <out>: GitHub API only, with the token.
    fetch_gh() {
        curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/octet-stream" -o "$2" "$1"
    }
    fetch_api() { # fetch_api <api.github.com url> -> stdout
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/vnd.github+json" "$1"
        else
            curl -fsSL -H "Accept: application/vnd.github+json" "$1"
        fi
    }
elif command -v wget >/dev/null 2>&1; then
    # wget re-sends the token to the S3 redirect, which S3 rejects.
    [ -z "${GITHUB_TOKEN:-}" ] || die "private-repo downloads (GITHUB_TOKEN) need curl"
    fetch() { wget -q -O "$2" "$1"; }
    fetch_api() { wget -q -O - "$1"; }
else
    die "need curl or wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "need sha256sum or shasum to verify the download"
fi

api="https://api.github.com/repos/$HOGLET_REPO"

if [ -z "$version" ]; then
    [ -z "${HOGLET_DOWNLOAD_URL:-}" ] || die "--version is required with HOGLET_DOWNLOAD_URL"
    version="$(fetch_api "$api/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$version" ] || die "could not find the latest release of $HOGLET_REPO (private repo? set GITHUB_TOKEN)"
fi
tag="v${version#v}"
version="${tag#v}"
asset="hoglet-$version-$os-$arch.tar.gz"

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t hoglet)"
trap 'rm -rf "$tmp"' EXIT INT TERM

download() { # download <asset-name> <out>
    if [ -n "${HOGLET_DOWNLOAD_URL:-}" ]; then
        fetch "${HOGLET_DOWNLOAD_URL%/}/$tag/$1" "$2"
    elif [ -n "${GITHUB_TOKEN:-}" ]; then
        # Private repos: browser_download_url 404s; resolve the API asset URL.
        # One JSON member per line; each asset lists "url" before "name".
        # shellcheck disable=SC2020 # three chars to three newlines, on purpose
        asset_url="$(fetch_api "$api/releases/tags/$tag" | tr ',{}' '\n\n\n' | sed 's/^[[:space:]]*//; s/":[[:space:]]*/":/' | \
            awk -v want="\"name\":\"$1\"" '
                /^"url":"https:\/\/api\.github\.com\/repos\/[^"]*\/releases\/assets\/[0-9]+"$/ { u = substr($0, 8, length($0) - 8) }
                $0 == want && u != "" { print u; exit }')"
        [ -n "$asset_url" ] || die "release $tag has no asset $1"
        fetch_gh "$asset_url" "$2"
    else
        fetch "https://github.com/$HOGLET_REPO/releases/download/$tag/$1" "$2"
    fi
}

say "downloading $asset ($tag)"
download "$asset" "$tmp/$asset" || die "download failed: $asset"
download "$asset.sha256" "$tmp/$asset.sha256" || die "download failed: $asset.sha256"

expected="$(cut -d' ' -f1 < "$tmp/$asset.sha256")"
actual="$(sha256 "$tmp/$asset")"
[ -n "$expected" ] && [ "$expected" = "$actual" ] || die "sha256 mismatch for $asset (expected $expected, got $actual)"
say "sha256 ok"

tar -xzf "$tmp/$asset" -C "$tmp"
src="$tmp/hoglet-$version-$os-$arch"
[ -x "$src/hoglet" ] || die "archive did not contain hoglet"

if [ -z "$prefix" ]; then
    if [ "$uid" -eq 0 ] || [ -w /usr/local/bin ]; then
        prefix=/usr/local/bin
    else
        prefix="$HOME/.local/bin"
    fi
fi

mkdir -p "$prefix"
# Write beside the target then rename: never leaves a half-written binary.
cp "$src/hoglet" "$prefix/.hoglet.new"
chmod 0755 "$prefix/.hoglet.new"
mv -f "$prefix/.hoglet.new" "$prefix/hoglet"
say "installed $prefix/hoglet ($tag)"

if [ "$systemd" -eq 1 ]; then
    if ! id hoglet >/dev/null 2>&1; then
        nologin="$(command -v nologin || echo /usr/sbin/nologin)"
        if command -v useradd >/dev/null 2>&1; then
            useradd --system --user-group --home-dir /var/lib/hoglet --no-create-home --shell "$nologin" hoglet
        elif command -v adduser >/dev/null 2>&1; then
            adduser -S -D -H -h /var/lib/hoglet -s "$nologin" hoglet
        else
            die "cannot create the hoglet user (no useradd/adduser)"
        fi
        say "created system user hoglet"
    fi
    install -m 0644 "$src/deploy/systemd/hoglet.service" /etc/systemd/system/hoglet.service
    # Keep the proxy examples and ops notes somewhere findable.
    rm -rf /usr/local/share/hoglet/deploy
    mkdir -p /usr/local/share/hoglet
    cp -R "$src/deploy" /usr/local/share/hoglet/deploy
    systemctl daemon-reload
    if systemctl is-active --quiet hoglet; then
        systemctl restart hoglet
        say "restarted hoglet.service"
    else
        systemctl enable --now hoglet
        say "enabled and started hoglet.service"
    fi
    say ""
    say "Hoglet listens on 127.0.0.1:8000 (data in /var/lib/hoglet)."
    say "Next: put Caddy or nginx in front for TLS — see /usr/local/share/hoglet/deploy/,"
    say "then open https://<your-host>/ to create the first account."
    say "Logs: journalctl -u hoglet -f"
    exit 0
fi

case ":$PATH:" in
    *":$prefix:"*) cmd=hoglet ;;
    *) cmd="$prefix/hoglet"; say "note: $prefix is not on your PATH" ;;
esac
say ""
say "Run it:"
say "  HOGLET_DATA=./hoglet-data $cmd"
say "then open http://127.0.0.1:8000/ to create the first account."
