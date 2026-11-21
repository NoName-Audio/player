#!/usr/bin/env bash
set -euo pipefail

# Usage: bash scripts/build-video-codecs.sh <FFmpeg source directory> <install prefix> <Rust target>
source_dir=$(cd "$1" && pwd)
mkdir -p "$2"
prefix=$(cd "$2" && pwd)
target=$3
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
cd "$build"

flags=()
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    arch=${target%%-*}
    sdk=$(xcrun --sdk macosx --show-sdk-path)
    apple_arch=$arch
    if [[ $arch == aarch64 ]]; then apple_arch=arm64; fi
    flags+=(--target-os=darwin --arch="$arch" --enable-cross-compile
      --cc="$(xcrun --find clang)" --sysroot="$sdk"
      --extra-cflags="-arch $apple_arch -mmacosx-version-min=11.0"
      --extra-ldflags="-arch $apple_arch -mmacosx-version-min=11.0")
    ;;
  x86_64-pc-windows-msvc)
    flags+=(--toolchain=msvc --arch=x86_64)
    ;;
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
    if [[ $(uname -m) != "${target%%-*}" ]]; then
      echo 'Linux requires a matching build host' >&2
      exit 2
    fi
    ;;
  *-linux-android*)
    : "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME}"
    case $(uname -s) in
      Darwin) host=darwin-x86_64 ;;
      Linux) host=linux-x86_64 ;;
      *) echo 'Use a macOS or Linux NDK host' >&2; exit 2 ;;
    esac
    bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin"
    case "$target" in
      aarch64-linux-android) arch=aarch64; triple=$target ;;
      armv7-linux-androideabi) arch=arm; triple=armv7a-linux-androideabi ;;
      i686-linux-android) arch=x86; triple=$target ;;
      x86_64-linux-android) arch=x86_64; triple=$target ;;
      *) exit 2 ;;
    esac
    flags+=(--target-os=android --arch="$arch" --enable-cross-compile
      --cc="$bin/${triple}24-clang" --ar="$bin/llvm-ar" --nm="$bin/llvm-nm"
      --ranlib="$bin/llvm-ranlib" --strip="$bin/llvm-strip")
    ;;
  *) echo "Unsupported Rust target: $target" >&2; exit 2 ;;
esac

"$source_dir/configure" --prefix="$prefix" "${flags[@]}" \
  --disable-gpl --disable-nonfree --disable-autodetect \
  --enable-static --disable-shared --enable-pic --disable-x86asm \
  --disable-programs --disable-doc --disable-avdevice --disable-avfilter \
  --disable-encoders --disable-muxers --disable-network --disable-protocols \
  --enable-protocol=file
make -j "${JOBS:-4}"
make install
