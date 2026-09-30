#!/usr/bin/env bash
# Copyright (C) 2026-2027 Zexshia
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
#
# Strip + UPX-compress the built workspace binaries for release packaging.
# Mirrors /usr/bin/compile-rust's tail end: llvm-strip (so the symbols the
# Android NDK toolchain recognises are the ones removed), then `upx
# --ultra-brute`. A stripped-then-UPX'd binary is what ships in the zip.
#
# Usage: .github/scripts/upx_compress.sh [profile]
#   profile: release (default) or debug
#
# Not a CI replacement for the build itself — build.yml calls this after
# `cargo ndk build` to shrink the artifacts it copies into mainfiles/.

set -euo pipefail

PROFILE="${1:-release}"
# This script lives in .github/scripts/, so the repo root is two levels up.
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

NDK_ROOT="${ANDROID_NDK_ROOT:-${NDK_ROOT:-/opt/android-sdk/ndk/28.2.13676358}}"
STRIP_BIN="$NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip"

# ABI target-triple -> the zip subdirectory `compile_zip.sh` writes into.
TARGETS=(
    "aarch64-linux-android:arm64-v8a"
    "armv7-linux-androideabi:armeabi-v7a"
)

# Binary target name -> the on-device filename it is installed as.
# The unified `sys.azenith-service` also answers for the symlinked names, but
# those are created by customize.sh at install time, not shipped as files.
# Cargo target names -> on-device names. `sys.azenith-service` is not a legal
# `[[bin]]` name (Cargo rejects `.`), so the rename happens here.
BINARIES=(
    "azenith-daemon:sys.azenith-service"
    "azenith-preloadbin:sys.azenith-preloadbin"
)

total_before=0
total_after=0

for target in "${TARGETS[@]}"; do
    triple="${target%%:*}"
    abi="${target##*:}"
    target_dir="$ROOT/target/$triple/$PROFILE"
    [ -d "$target_dir" ] || continue

    for entry in "${BINARIES[@]}"; do
        bin="${entry%%:*}"
        outname="${entry##*:}"
        [ -f "$target_dir/$bin" ] || continue

        before=$(stat -c%s "$target_dir/$bin")
        if [ -x "$STRIP_BIN" ]; then
            "$STRIP_BIN" "$target_dir/$bin"
        else
            strip "$target_dir/$bin"
        fi

        if command -v upx >/dev/null 2>&1; then
            # --ultra-brute is slow; a failed compression must not fail the
            # build, the un-UPX'd stripped binary is still shippable.
            upx --ultra-brute -q "$target_dir/$bin" || \
                echo ">>> warning: UPX skipped $abi/$bin" >&2
        else
            echo ">>> warning: upx not found; shipping $abi/$bin un-compressed" >&2
        fi

        after=$(stat -c%s "$target_dir/$bin")
        total_before=$((total_before + before))
        total_after=$((total_after + after))
        printf '>>> %-16s %-28s %6d KiB -> %6d KiB (%d%%)\n' \
            "$abi" "$outname" \
            "$((before / 1024))" "$((after / 1024))" \
            "$((100 * after / (before > 0 ? before : 1)))"
    done
done

if [ "$total_before" -gt 0 ]; then
    printf '>>> TOTAL: %d KiB -> %d KiB (%d%% of original)\n' \
        "$((total_before / 1024))" "$((total_after / 1024))" \
        "$((100 * total_after / total_before))"
else
    echo ">>> no release binaries found under target/*/$PROFILE" >&2
    exit 1
fi
