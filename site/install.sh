#!/bin/sh
# poenv installer for macOS and Linux.
#
#   curl -fsSL https://poenv.pogi.codes/install.sh | sh
#   curl -fsSL https://poenv.pogi.codes/install.sh | sh -s -- --yes --version 0.0.2
#
# Options (or env vars):
#   --version X        POENV_VERSION         install X instead of the latest release
#   --dir PATH         POENV_INSTALL_DIR     where to put the binary (default ~/.local/bin)
#   --no-modify-path   POENV_NO_MODIFY_PATH  don't add the dir to your shell's PATH
#   --yes                                    never prompt (for agents and scripts; it never prompts anyway)
#
# No sudo, no root: everything goes in your home directory.
set -eu

REPO="pogicodes/poenv"
VERSION="${POENV_VERSION:-latest}"
DIR="${POENV_INSTALL_DIR:-$HOME/.local/bin}"
MODIFY_PATH="${POENV_NO_MODIFY_PATH:+no}"
MODIFY_PATH="${MODIFY_PATH:-yes}"
# Where release files come from. Overridable for testing (a URL or a local directory).
BASE="${POENV_DOWNLOAD_BASE:-}"

# Same look as the CLI: color only on a terminal, and never with NO_COLOR.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  esc=$(printf '\033')
  bold="$esc[1m" dim="$esc[2m" green="$esc[32m" yellow="$esc[33m" red="$esc[31m" reset="$esc[0m"
else
  bold="" dim="" green="" yellow="" red="" reset=""
fi
say() { printf '%s\n' "$*"; }
ok() { say "${green}✓${reset} $*"; }
warn() { say "${yellow}!${reset} $*"; }
fail() { printf '%spoenv install:%s %s\n' "$red" "$reset" "$*" >&2; exit 1; }
# ~ in place of $HOME, for display only.
pretty() { case "$1" in "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;; *) printf '%s' "$1" ;; esac; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --dir) DIR="${2:?--dir needs a value}"; shift 2 ;;
    --no-modify-path) MODIFY_PATH=no; shift ;;
    --yes|-y) shift ;;
    *) fail "unknown option: $1" ;;
  esac
done

# --- platform ---
os=$(uname -s)
case "$os" in
  Darwin) os=darwin ;;
  Linux) os=linux ;;
  *) fail "unsupported OS: $os (on Windows, use install.ps1)" ;;
esac
arch=$(uname -m)
case "$arch" in
  x86_64|amd64) arch=x64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) fail "unsupported CPU: $arch" ;;
esac
# An x64 shell under Rosetta on Apple Silicon: install the native build.
if [ "$os" = darwin ] && [ "$arch" = x64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch=arm64
fi
asset="poenv-$os-$arch"

if [ -z "$BASE" ]; then
  if [ "$VERSION" = latest ]; then
    # Resolve the tag first: GitHub caches the releases/latest/download/... redirect for a while
    # after a release, while the releases/latest page itself updates right away.
    if command -v curl >/dev/null 2>&1; then
      url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") || url=""
      case "$url" in */tag/v*) VERSION="${url##*/tag/v}" ;; esac
    fi
  fi
  if [ "$VERSION" = latest ]; then
    BASE="https://github.com/$REPO/releases/latest/download"
  else
    BASE="https://github.com/$REPO/releases/download/v${VERSION#v}"
  fi
fi

# --- download ---
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch() { # fetch NAME DEST
  case "$BASE" in
    http://*|https://*)
      if command -v curl >/dev/null 2>&1; then curl -fsSL "$BASE/$1" -o "$2"
      elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$BASE/$1"
      else fail "needs curl or wget"
      fi ;;
    *) cp "$BASE/$1" "$2" ;;
  esac
}

say "${dim}Downloading poenv $VERSION ($os-$arch)...${reset}"
fetch "$asset" "$tmp/poenv" || fail "download failed: $BASE/$asset"
fetch SHA256SUMS "$tmp/SHA256SUMS" || fail "download failed: $BASE/SHA256SUMS"

# --- verify ---
expected=$(awk -v f="$asset" '$2 == f || $2 == "*"f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "no checksum for $asset in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$tmp/poenv" | awk '{print $1}')
else actual=$(shasum -a 256 "$tmp/poenv" | awk '{print $1}')
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset (expected $expected, got $actual)"

# --- install ---
mkdir -p "$DIR"
chmod +x "$tmp/poenv"
mv -f "$tmp/poenv" "$DIR/poenv"
installed=$("$DIR/poenv" --version 2>/dev/null) || fail "installed, but $DIR/poenv doesn't run"
ok "Installed ${bold}poenv $installed${reset} to $(pretty "$DIR/poenv")"

# --- PATH ---
case ":$PATH:" in
  *":$DIR:"*) on_path=yes ;;
  *) on_path=no ;;
esac
if [ "$on_path" = no ]; then
  line="export PATH=\"$DIR:\$PATH\""
  case "${SHELL:-}" in
    */zsh) rc="$HOME/.zshrc" ;;
    */bash) if [ "$os" = darwin ]; then rc="$HOME/.bash_profile"; else rc="$HOME/.bashrc"; fi ;;
    */fish) rc="" ; line="fish_add_path $DIR" ;;
    *) rc="$HOME/.profile" ;;
  esac
  if [ "$MODIFY_PATH" = yes ] && [ -n "$rc" ]; then
    if ! grep -qs "# added by poenv installer" "$rc"; then
      printf '\n%s # added by poenv installer\n' "$line" >> "$rc"
    fi
    ok "Added $(pretty "$DIR") to PATH in $(pretty "$rc")"
    say "  ${dim}Open a new terminal, or run:${reset} $line"
  else
    warn "$(pretty "$DIR") isn't on your PATH. Add it with:"
    say "  $line"
  fi
fi

say ""
say "${bold}Get started${reset}"
say "  ${bold}cd your-repo && poenv init${reset}    ${dim}import the repo's .env into your vault${reset}"
say "  ${bold}poenv env new staging${reset}         ${dim}add another env${reset}"
say "  ${bold}poenv use staging${reset}             ${dim}switch the repo's .env to it${reset}"
say "  ${bold}poenv${reset}                         ${dim}browse and edit everything${reset}"
say ""
say "${dim}Stuck? ${reset}${bold}poenv doctor${reset}${dim} checks your setup. All commands: ${reset}${bold}poenv --help${reset}"
