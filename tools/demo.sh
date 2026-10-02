#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash tools/build.sh
python3 tools/media.py
python3 tools/server.py
