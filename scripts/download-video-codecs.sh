#!/usr/bin/env bash
set -euo pipefail
archive=${1:?Specify output archive path}
curl -fL --retry 3 https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz -o "$archive"
node -e '
  const { createHash } = require("node:crypto");
  const { createReadStream } = require("node:fs");
  const hash = createHash("sha256");
  createReadStream(process.argv[1]).on("data", chunk => hash.update(chunk)).on("end", () => {
    if (hash.digest("hex") !== "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e") {
      throw new Error("FFmpeg archive checksum mismatch");
    }
  });
' "$archive"
