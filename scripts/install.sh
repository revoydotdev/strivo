#!/usr/bin/env bash
# Portable source installer for Strivo.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
EDITION="pvr"
PROFILE="release"
PREFIX="${STRIVO_PREFIX:-${HOME:?}/.local}"
ACTION="install"

usage() {
    printf '%s\n' \
        "Usage: scripts/install.sh [options]" \
        "" \
        "  --edition pvr|creator  Product edition (default: pvr)" \
        "  --debug                Faster, unoptimized development build" \
        "  --prefix PATH          Install prefix (default: ~/.local)" \
        "  --check                Check prerequisites without building" \
        "  --uninstall            Remove installed program files; keep user data" \
        "  -h, --help             Show this help" \
        "" \
        "Environment: STRIVO_PREFIX, CARGO"
}

while (($#)); do
    case "$1" in
        --edition)
            (($# >= 2)) || { echo "missing value for --edition" >&2; exit 2; }
            EDITION="$2"; shift 2 ;;
        --debug) PROFILE="debug"; shift ;;
        --release) PROFILE="release"; shift ;;
        --prefix)
            (($# >= 2)) || { echo "missing value for --prefix" >&2; exit 2; }
            PREFIX="$2"; shift 2 ;;
        --check) ACTION="check"; shift ;;
        --uninstall) ACTION="uninstall"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

case "$EDITION" in pvr|creator) ;; *) echo "--edition must be pvr or creator" >&2; exit 2 ;; esac

BIN_DIR="${STRIVO_BIN_DIR:-$PREFIX/bin}"
SHARE_DIR="${STRIVO_SHARE_DIR:-$PREFIX/share/strivo}"
MAN_DIR="${STRIVO_MAN_DIR:-$PREFIX/share/man/man1}"
BIN_PATH="$BIN_DIR/strivo"
CARGO_BIN="${CARGO:-cargo}"

if [[ "$ACTION" == "uninstall" ]]; then
    rm -f "$BIN_PATH" "$BIN_DIR/whisperx_diarize.py" "$MAN_DIR/strivo.1"
    rm -rf "$SHARE_DIR/completions"
    printf '✓ Removed Strivo program files. Configuration and recordings were preserved.\n'
    exit 0
fi

missing=()
for tool in "$CARGO_BIN" ffmpeg mpv streamlink yt-dlp; do
    command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if ((${#missing[@]})); then
    printf 'Missing prerequisites: %s\n' "${missing[*]}" >&2
    case "$(uname -s)" in
        Darwin) printf 'Install them with Homebrew; install Rust via https://rustup.rs\n' >&2 ;;
        Linux)
            if command -v apt-get >/dev/null 2>&1; then
                printf 'Debian/Ubuntu: sudo apt install ffmpeg mpv python3-pip && pipx install streamlink yt-dlp\n' >&2
            elif command -v dnf >/dev/null 2>&1; then
                printf 'Fedora: sudo dnf install ffmpeg mpv pipx && pipx install streamlink yt-dlp\n' >&2
            elif command -v pacman >/dev/null 2>&1; then
                printf 'Arch: sudo pacman -S rust ffmpeg mpv streamlink yt-dlp\n' >&2
            else
                printf 'Install Rust via https://rustup.rs plus ffmpeg, mpv, streamlink, and yt-dlp.\n' >&2
            fi ;;
    esac
    exit 1
fi
printf '✓ Prerequisites found\n'
[[ "$ACTION" == "check" ]] && exit 0

cd "$REPO_ROOT"
build_args=(build --locked -p strivo-bin)
[[ "$PROFILE" == "release" ]] && build_args+=(--release)
[[ "$EDITION" == "creator" ]] && build_args+=(--features creator)
printf '› Building Strivo %s (%s)\n' "$EDITION" "$PROFILE"
"$CARGO_BIN" "${build_args[@]}"

BUILT_BIN="$REPO_ROOT/target/$PROFILE/strivo"
[[ -x "$BUILT_BIN" ]] || { echo "build completed but binary is missing" >&2; exit 1; }
install -d "$BIN_DIR" "$SHARE_DIR/completions" "$MAN_DIR"
install -m 0755 "$BUILT_BIN" "$BIN_PATH"

if [[ "$EDITION" == "creator" ]]; then
    sidecar="$REPO_ROOT/crates/strivo-plugins/scripts/whisperx_diarize.py"
    [[ -f "$sidecar" ]] && install -m 0755 "$sidecar" "$BIN_DIR/whisperx_diarize.py"
fi

for shell in bash zsh fish; do
    "$BIN_PATH" completions "$shell" > "$SHARE_DIR/completions/strivo.$shell"
done
"$BIN_PATH" man > "$MAN_DIR/strivo.1"

printf '✓ Installed %s\n' "$BIN_PATH"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) printf '  Add %s to PATH.\n' "$BIN_DIR" ;; esac
printf '  1. Run: strivo doctor\n'
printf '  2. Optional browser login import: strivo setup cookies youtube --browser firefox\n'
printf '  3. Run: strivo  (then open http://127.0.0.1:8181)\n'

