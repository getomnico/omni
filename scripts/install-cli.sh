#!/usr/bin/env sh
set -eu

REPO="${OMNI_CLI_REPO:-getomnico/omni}"
VERSION="${OMNI_CLI_VERSION:-latest}"
if [ -n "${OMNI_CLI_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$OMNI_CLI_INSTALL_DIR"
elif [ -n "${XDG_BIN_HOME:-}" ]; then
  INSTALL_DIR="$XDG_BIN_HOME"
elif [ -n "${HOME:-}" ]; then
  INSTALL_DIR="$HOME/.local/bin"
else
  INSTALL_DIR=""
fi
BINARY_NAME="${OMNI_CLI_BINARY_NAME:-omni}"

usage() {
  cat <<'EOF'
Install the Omni CLI from GitHub release artifacts.

Usage:
  install-cli.sh [options]

Options:
  --version VERSION      Release tag to install, e.g. v0.1.7 or 0.1.7 (default: latest)
  --repo OWNER/REPO      GitHub repository to download from (default: getomnico/omni)
  --install-dir PATH     Directory to install into (default: $XDG_BIN_HOME or $HOME/.local/bin)
  --binary-name NAME     Installed binary name (default: omni)
  -h, --help             Show this help

Environment overrides:
  OMNI_CLI_VERSION, OMNI_CLI_REPO, OMNI_CLI_INSTALL_DIR, OMNI_CLI_BINARY_NAME
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --repo)
      REPO="${2:-}"
      shift 2
      ;;
    --install-dir)
      INSTALL_DIR="${2:-}"
      shift 2
      ;;
    --binary-name)
      BINARY_NAME="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [ -z "$VERSION" ] || [ -z "$REPO" ] || [ -z "$INSTALL_DIR" ] || [ -z "$BINARY_NAME" ]; then
  echo "error: version, repo, install-dir, and binary-name must be non-empty" >&2
  exit 1
fi

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

detect_asset() {
  os="$(uname -s 2>/dev/null || true)"
  arch="$(uname -m 2>/dev/null || true)"

  case "$os" in
    Linux)
      os_part="linux"
      ;;
    Darwin)
      os_part="macos"
      ;;
    *)
      echo "error: unsupported OS: ${os:-unknown}; supported: Linux, macOS" >&2
      exit 1
      ;;
  esac

  case "$arch" in
    x86_64|amd64)
      arch_part="x86_64"
      ;;
    arm64|aarch64)
      arch_part="arm64"
      ;;
    *)
      echo "error: unsupported architecture: ${arch:-unknown}; supported: x86_64, arm64" >&2
      exit 1
      ;;
  esac

  printf 'omni-%s-%s' "$os_part" "$arch_part"
}

download() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$dest"
  else
    echo "error: curl or wget is required" >&2
    exit 1
  fi
}

try_download() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$dest"
  else
    echo "error: curl or wget is required" >&2
    exit 1
  fi
}

verify_checksum() {
  dir="$1"
  checksum_file="$2"

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dir" && sha256sum -c "$checksum_file")
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$dir" && shasum -a 256 -c "$checksum_file")
  else
    echo "warning: sha256sum or shasum not found; skipping checksum verification" >&2
  fi
}

install_binary() {
  src="$1"
  dest_dir="$2"
  dest="$dest_dir/$BINARY_NAME"

  if [ -d "$dest_dir" ] && [ -w "$dest_dir" ]; then
    cp "$src" "$dest"
    chmod 0755 "$dest"
  elif [ ! -e "$dest_dir" ]; then
    parent="$(dirname "$dest_dir")"
    if [ -w "$parent" ]; then
      mkdir -p "$dest_dir"
      cp "$src" "$dest"
      chmod 0755 "$dest"
    elif command -v sudo >/dev/null 2>&1; then
      sudo mkdir -p "$dest_dir"
      sudo cp "$src" "$dest"
      sudo chmod 0755 "$dest"
    else
      echo "error: $dest_dir is not writable and sudo is unavailable" >&2
      exit 1
    fi
  elif command -v sudo >/dev/null 2>&1; then
    sudo cp "$src" "$dest"
    sudo chmod 0755 "$dest"
  else
    echo "error: $dest_dir is not writable and sudo is unavailable" >&2
    exit 1
  fi

  echo "Installed $BINARY_NAME to $dest"
}

ASSET_NAME="$(detect_asset)"
CHECKSUM_NAME="$ASSET_NAME.sha256"

case "$VERSION" in
  latest)
    BASE_URL="https://github.com/$REPO/releases/latest/download"
    ;;
  v*)
    BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
    ;;
  *)
    BASE_URL="https://github.com/$REPO/releases/download/v$VERSION"
    ;;
esac

require_cmd uname
require_cmd chmod
require_cmd cp
require_cmd mkdir

TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t omni-cli)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

ASSET_PATH="$TMP_DIR/$ASSET_NAME"
CHECKSUM_PATH="$TMP_DIR/$CHECKSUM_NAME"

ASSET_URL="$BASE_URL/$ASSET_NAME"
CHECKSUM_URL="$BASE_URL/$CHECKSUM_NAME"

echo "Downloading $ASSET_URL"
download "$ASSET_URL" "$ASSET_PATH"

if try_download "$CHECKSUM_URL" "$CHECKSUM_PATH" 2>/dev/null; then
  verify_checksum "$TMP_DIR" "$CHECKSUM_NAME"
else
  echo "warning: checksum not found at $CHECKSUM_URL; skipping checksum verification" >&2
fi

chmod +x "$ASSET_PATH"
install_binary "$ASSET_PATH" "$INSTALL_DIR"

if command -v "$BINARY_NAME" >/dev/null 2>&1; then
  "$BINARY_NAME" --version || true
else
  echo "Note: $INSTALL_DIR is not on PATH. Add it to PATH or run $INSTALL_DIR/$BINARY_NAME directly."
fi
