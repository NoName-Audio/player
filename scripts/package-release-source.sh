#!/usr/bin/env bash
set -euo pipefail
output=${1:?Specify release artifact directory}
mkdir -p "$output"
output=$(cd "$output" && pwd)
cd "$(dirname "$0")/.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export SOURCE_STAGE="$work/player-source"
mkdir -p "$SOURCE_STAGE/player"
# Archive only tracked release content, not build products, secrets, or local edits.
git archive HEAD | tar -xf - -C "$SOURCE_STAGE/player"
submodules=$(git submodule status --recursive)
if [[ $submodules =~ (^|$'\n')[-+U] ]]; then
  echo 'Check out all pinned submodule revisions recursively before packaging.' >&2
  exit 1
fi
git submodule foreach --quiet --recursive '
  mkdir -p "$SOURCE_STAGE/player/$displaypath" &&
  git archive "$sha1" | tar -xf - -C "$SOURCE_STAGE/player/$displaypath"
'
bash scripts/download-video-codecs.sh "$SOURCE_STAGE/ffmpeg-9.0.2.tar.xz"
tar -xOf "$SOURCE_STAGE/ffmpeg-9.0.2.tar.xz" ffmpeg-9.0.2/COPYING.LGPLv2.1 > "$SOURCE_STAGE/FFmpeg-COPYING.LGPLv2.1"
git rev-parse HEAD > "$SOURCE_STAGE/player-commit.txt"
tar -czf "$work/player-corresponding-source.tar.gz" -C "$work" player-source
mv "$work/player-corresponding-source.tar.gz" "$output/"
