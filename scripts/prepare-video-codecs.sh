#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
prefix=$1
shift
mkdir -p "$prefix"
prefix=$(cd "$prefix" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [[ -n ${FFMPEG_SOURCE_DIR:-} ]]; then
  source_dir=$(cd "$FFMPEG_SOURCE_DIR" && pwd)
else
  archive="$work/ffmpeg-9.0.2.tar.xz"
  bash "$root/scripts/download-video-codecs.sh" "$archive"
  tar -xf "$archive" -C "$work"
  source_dir="$work/ffmpeg-9.0.2"
fi
mkdir -p "$prefix/lib"
for target in "$@"; do
  bash "$root/scripts/build-video-codecs.sh" "$source_dir" "$prefix/$target" "$target"
  case "$target" in
    aarch64-*) libdir=lib/arm64 ;;
    x86_64-*) libdir=lib/amd64 ;;
    armv7-*) libdir=lib/armhf ;;
    i686-*) libdir=lib ;;
  esac
  mkdir -p "$prefix/$libdir"
  cp "$prefix/$target/lib/"*.a "$prefix/$libdir/" 2>/dev/null || cp "$prefix/$target/lib/"*.lib "$prefix/$libdir/"
  cp -R "$prefix/$target/include" "$prefix/"
done

# These software-only builds do not provide Vulkan. On 32-bit Android,
# ffmpeg-sys-next 9.1.1's optional Vulkan bindings assume a 64-bit ABI.
for target in "$@"; do
  case "$target" in
    armv7-linux-androideabi|i686-linux-android)
      rm -f "$prefix/include/libavutil/hwcontext_vulkan.h"
      break
      ;;
  esac
done
