#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash tools/build.sh
python3 tools/media.py
for file in tools/media/aac.ts tools/media/ac3.ts; do
  for scenario in demux stream push; do node tools/node/profile.cjs "$scenario" "$file"; done
done
for file in tools/media/movie.mp4 tools/media/movie.mkv tools/media/movie-aac.mp4; do
  for scenario in index movie; do node tools/node/profile.cjs "$scenario" "$file"; done
done

node tools/node/profile.cjs flac tools/media/programme.ts
