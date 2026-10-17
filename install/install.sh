#!/bin/sh
# pgcli (PostgreSQL CLI client) -- Linux and macOS installer
#
# Usage (one-liner):
#   curl -fsSL https://raw.githubusercontent.com/jhonsferg/pgcli-rs/main/install/install.sh | sh
#
# Customise with environment variables on the shell side of the pipe:
#   curl -fsSL ... | PGCLI_INSTALL_DIR=$HOME/bin PGCLI_VERSION=v1.1.3 sh
#
# Variables:
#   PGCLI_VERSION        release tag to install (default: latest)
#   PGCLI_INSTALL_DIR    install directory (default: $HOME/.local/bin)
#   PGCLI_REPO           owner/repo to download from (default: jhonsferg/pgcli-rs)
#   PGCLI_TEST_API_BASE  replaces https://api.github.com (testing only)
#   PGCLI_TEST_DL_BASE   replaces https://github.com (testing only)
#   PGCLI_TEST_ALLOW_HTTP=1  permits http:// test bases (testing only)
#
# The checksum in checksums.txt is mandatory: if it is missing or does not
# list the archive, nothing is installed.
#
# This script never needs root: it installs under the user's home directory.

set -eu

REPO="${PGCLI_REPO:-jhonsferg/pgcli-rs}"
INSTALL_DIR="${PGCLI_INSTALL_DIR:-$HOME/.local/bin}"
PGCLI_VERSION="${PGCLI_VERSION:-latest}"

API_BASE="${PGCLI_TEST_API_BASE:-https://api.github.com}"
DL_BASE="${PGCLI_TEST_DL_BASE:-https://github.com}"

# -- Terminal helpers ----------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_CYAN='\033[0;36m'
    C_GREEN='\033[0;32m'
    C_YELLOW='\033[1;33m'
    C_RED='\033[0;31m'
    C_BOLD='\033[1m'
    C_RESET='\033[0m'
else
    C_CYAN='' C_GREEN='' C_YELLOW='' C_RED='' C_BOLD='' C_RESET=''
fi

step() { printf "  ${C_CYAN}->${C_RESET} %s\n" "$1"; }
ok()   { printf "  ${C_GREEN}v ${C_RESET} %s\n" "$1"; }
warn() { printf "  ${C_YELLOW}! ${C_RESET} %s\n" "$1" >&2; }
die()  { printf "  ${C_RED}x ${C_RESET} %b\n" "$1" >&2; exit 1; }

printf "\n  ${C_BOLD}${C_CYAN}pgcli${C_RESET}${C_BOLD} -- PostgreSQL CLI client${C_RESET} installer\n\n"

# -- 1. Check required tools ---------------------------------------------------
need() {
    command -v "$1" > /dev/null 2>&1 || die "'$1' is required but not installed."
}
need curl
need tar
need awk
need mktemp

# Computes the SHA-256 digest of $1, preferring GNU coreutils' sha256sum and
# falling back to macOS/BSD's shasum.
sha256_of() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum > /dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "Neither 'sha256sum' nor 'shasum' is available to verify the download."
    fi
}

# -- 2. Validate inputs --------------------------------------------------------
# Only https:// endpoints are accepted. A plain http:// base is allowed only
# when PGCLI_TEST_ALLOW_HTTP=1 is set explicitly, for local test servers.
CURL_PROTO="=https"
case "$API_BASE" in
    https://*) ;;
    http://*)
        [ "${PGCLI_TEST_ALLOW_HTTP:-}" = "1" ] \
            || die "PGCLI_TEST_API_BASE must use https:// (http:// needs PGCLI_TEST_ALLOW_HTTP=1)."
        CURL_PROTO="=http,https"
        ;;
    *) die "PGCLI_TEST_API_BASE must start with https://" ;;
esac
case "$DL_BASE" in
    https://*) ;;
    http://*)
        [ "${PGCLI_TEST_ALLOW_HTTP:-}" = "1" ] \
            || die "PGCLI_TEST_DL_BASE must use https:// (http:// needs PGCLI_TEST_ALLOW_HTTP=1)."
        CURL_PROTO="=http,https"
        ;;
    *) die "PGCLI_TEST_DL_BASE must start with https://" ;;
esac
if [ "$API_BASE" != "https://api.github.com" ] || [ "$DL_BASE" != "https://github.com" ]; then
    warn "Using test endpoints: API=$API_BASE DL=$DL_BASE"
fi

# -- 3. Detect OS and architecture ---------------------------------------------
OS="$(uname -s 2>/dev/null || echo unknown)"
case "$OS" in
    Linux)  PLATFORM="linux"  ;;
    Darwin) PLATFORM="darwin" ;;
    *)      die "Unsupported OS: $OS  (only Linux and macOS are supported)" ;;
esac

# Termux (Android) ships a Linux kernel but uses the android build (bionic).
if [ "$PLATFORM" = "linux" ] && { [ -n "${TERMUX_VERSION:-}" ] || [ -d "/data/data/com.termux" ]; }; then
    PLATFORM="android"
fi

MACHINE="$(uname -m 2>/dev/null || echo unknown)"
case "$PLATFORM-$MACHINE" in
    linux-x86_64 | linux-amd64)             ARCH="x86_64"  ;;
    linux-aarch64 | linux-arm64)            ARCH="aarch64" ;;
    linux-armv7*)                           ARCH="armv7"   ;;
    linux-i686 | linux-i386 | linux-i586)  ARCH="386"     ;;
    linux-riscv64)                          ARCH="riscv64" ;;
    linux-s390x)                            ARCH="s390x"   ;;
    linux-ppc64le)                          ARCH="ppc64le" ;;
    android-aarch64 | android-arm64)        ARCH="aarch64" ;;
    darwin-x86_64)                          ARCH="x86_64"  ;;
    darwin-arm64 | darwin-aarch64)          ARCH="aarch64" ;;
    *) die "No pgcli binary is published for $OS $MACHINE." ;;
esac

step "Detected platform: $PLATFORM-$ARCH"

# -- 4. Resolve version --------------------------------------------------------
if [ "$PGCLI_VERSION" = "latest" ]; then
    step "Fetching latest release from $API_BASE..."
    API_RESPONSE="$(curl -sSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" \
        --retry 3 --max-time 30 -w '\n%{http_code}' \
        "$API_BASE/repos/$REPO/releases/latest")" \
        || die "Could not reach $API_BASE. Check your internet connection."
    API_STATUS="$(printf '%s' "$API_RESPONSE" | tail -n 1)"
    API_BODY="$(printf '%s' "$API_RESPONSE" | sed '$d')"
    if [ "$API_STATUS" = "403" ]; then
        die "GitHub API rate limit exceeded (HTTP 403). Try again later, or set PGCLI_VERSION explicitly."
    fi
    if [ "$API_STATUS" != "200" ]; then
        die "Failed to resolve the latest version (HTTP $API_STATUS)."
    fi
    PGCLI_VERSION="$(printf '%s' "$API_BODY" | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
    [ -n "$PGCLI_VERSION" ] || die "Failed to parse the latest version from the GitHub API response."
fi

case "$PGCLI_VERSION" in
    v*) ;;
    *) PGCLI_VERSION="v$PGCLI_VERSION" ;;
esac
printf '%s' "$PGCLI_VERSION" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' \
    || die "Invalid version '$PGCLI_VERSION'. Expected a tag such as v0.1.0."

step "Installing pgcli $PGCLI_VERSION"

# -- 5. Prepare staging area ---------------------------------------------------
ARCHIVE="pgcli-rs_${PLATFORM}_${ARCH}.tar.gz"
URL="$DL_BASE/$REPO/releases/download/$PGCLI_VERSION/$ARCHIVE"
CHECKSUMS_URL="$DL_BASE/$REPO/releases/download/$PGCLI_VERSION/checksums.txt"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pgcli-install.XXXXXX")"
STAGE="$INSTALL_DIR/.pgcli-install.$$"
trap 'rm -rf "$TMP_DIR"; rm -f "$STAGE"' EXIT INT TERM HUP

# -- 6. Fetch and parse checksums.txt (mandatory) -----------------------------
# Checksums are fetched first: if they are missing, nothing is downloaded or
# installed.
CHECKSUMS_FILE="$TMP_DIR/checksums.txt"
step "Fetching checksums.txt..."
if ! curl -fsSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" \
        --retry 3 --max-time 30 "$CHECKSUMS_URL" -o "$CHECKSUMS_FILE"; then
    die "checksums.txt is not available for $PGCLI_VERSION; refusing to install without verification.\n  URL: $CHECKSUMS_URL"
fi

ENTRIES="$(awk -v f="$ARCHIVE" '$2 == f {print $1}' "$CHECKSUMS_FILE")"
ENTRY_COUNT="$(printf '%s' "$ENTRIES" | awk 'NF { n++ } END { print n + 0 }')"
[ "$ENTRY_COUNT" = "1" ] \
    || die "Expected exactly one checksum entry for $ARCHIVE in checksums.txt, found $ENTRY_COUNT."
EXPECTED_SHA="$ENTRIES"
printf '%s' "$EXPECTED_SHA" | grep -Eq '^[0-9a-f]{64}$' \
    || die "Malformed checksum entry for $ARCHIVE in checksums.txt."

# -- 7. Download and verify archive -------------------------------------------
ARCHIVE_FILE="$TMP_DIR/$ARCHIVE"
step "Downloading $ARCHIVE..."
if ! curl -fSL --proto "$CURL_PROTO" --proto-redir "$CURL_PROTO" \
        --retry 3 --retry-delay 2 --max-time 300 --progress-bar "$URL" -o "$ARCHIVE_FILE"; then
    die "Download failed.\n  URL: $URL\n  Check that release $PGCLI_VERSION exists."
fi

step "Verifying checksum..."
ACTUAL_SHA="$(sha256_of "$ARCHIVE_FILE")"
if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
    die "Checksum mismatch for $ARCHIVE; nothing was installed.\n  expected: $EXPECTED_SHA\n  got:      $ACTUAL_SHA"
fi
ok "Checksum verified"

# -- 8. Extract and install binary ---------------------------------------------
EXTRACT_DIR="$TMP_DIR/extract"
mkdir "$EXTRACT_DIR"
step "Extracting..."
if ! tar -xzf "$ARCHIVE_FILE" -C "$EXTRACT_DIR" pgcli; then
    die "Extraction failed. The archive may be corrupted."
fi
[ -f "$EXTRACT_DIR/pgcli" ] || die "Archive did not contain the pgcli binary."

mkdir -p "$INSTALL_DIR"
# Copy to a staging name in the destination directory, then rename over the
# target: the rename is atomic, so an interrupted install never leaves a
# half-written binary behind.
cp "$EXTRACT_DIR/pgcli" "$STAGE"
chmod 755 "$STAGE"
mv -f "$STAGE" "$INSTALL_DIR/pgcli"
ok "Installed to $INSTALL_DIR/pgcli"

if ! "$INSTALL_DIR/pgcli" --version > /dev/null 2>&1; then
    die "Installed binary failed to run: $INSTALL_DIR/pgcli --version"
fi

# -- 9. Summary -------------------------------------------------------------------
# pgcli is a single binary: it needs no shell integration, profile block or
# registry entry. The installer only reports whether the directory is on PATH.
printf "\n  ${C_GREEN}${C_BOLD}pgcli $PGCLI_VERSION installed!${C_RESET}\n\n"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf "  ${C_YELLOW}Note:${C_RESET} %s is not in your PATH.\n" "$INSTALL_DIR"
        printf "  Run it as %s/pgcli, or add the directory to PATH in your shell profile:\n" "$INSTALL_DIR"
        printf "       ${C_CYAN}export PATH=\"%s:\$PATH\"${C_RESET}\n\n" "$INSTALL_DIR"
        ;;
esac

printf "  Connect to a database:\n"
printf "       ${C_CYAN}pgcli postgresql://user@localhost/dbname${C_RESET}\n\n"
printf "  Upgrade later with:\n"
printf "       ${C_CYAN}pgcli upgrade${C_RESET}\n\n"
