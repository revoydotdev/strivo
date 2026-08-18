#!/usr/bin/env bash
# Cross-check the workspace for Windows without a Windows machine.
#
# Why the GNU target and not MSVC: `cargo xwin` needs to download the MSVC CRT,
# which is not always reachable, and a plain `--target x86_64-pc-windows-msvc`
# check fails in `ring` and `libsqlite3-sys` build scripts without an MSVC
# toolchain. The GNU target builds both of those with the mingw-w64 gcc that is
# already a normal Linux package, so it is the one loop that works everywhere.
#
# WHAT THIS CATCHES: every `cfg`-gated API error — missing `tokio::net::Unix*`,
# `std::os::unix`, Unix-only libc calls. That is the bulk of a port.
#
# WHAT IT DOES NOT CATCH: MSVC-specific linking, ABI differences, and anything
# that only shows up at runtime. CI and the release artifacts both ship
# x86_64-pc-windows-msvc, so this is the development loop, NOT the gate. The
# gate is a real MSVC build on the win11-ci runner.
#
# Setup (once):
#   rustup target add x86_64-pc-windows-gnu
#   # Arch: pacman -S mingw-w64-gcc     Debian: apt install gcc-mingw-w64
set -euo pipefail

TARGET=x86_64-pc-windows-gnu

if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "error: rust target $TARGET is not installed" >&2
    echo "  rustup target add $TARGET" >&2
    exit 1
fi
if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
    echo "error: mingw-w64 gcc not found (needed to build ring/libsqlite3-sys)" >&2
    echo "  Arch: pacman -S mingw-w64-gcc   Debian/Ubuntu: apt install gcc-mingw-w64" >&2
    exit 1
fi

echo "checking workspace for $TARGET ..."
cargo check --workspace --all-targets --target "$TARGET" --locked "$@"
