#!/usr/bin/env bash
# Compare two APK payloads after excluding META-INF/* (signing files).
# Exit 2 when the two APK paths are missing.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "apk-payload: need two apk paths; signing entries under META-INF/ are excluded" >&2
  exit 2
fi

python3 - "$1" "$2" <<'PY'
import sys
import zipfile
from pathlib import Path

def names(path):
    if not Path(path).is_file():
        print(f"apk-payload: missing {path}", file=sys.stderr)
        sys.exit(2)
    with zipfile.ZipFile(path) as archive:
        found = [
            name
            for name in archive.namelist()
            if name != "META-INF/" and not name.startswith("META-INF/")
        ]
    return sorted(found)

left, right = names(sys.argv[1]), names(sys.argv[2])
if left != right:
    print("apk-payload: payload entries differ after the META-INF/ exclusion", file=sys.stderr)
    for name in left:
        if name not in right:
            print(f"only left: {name}", file=sys.stderr)
    for name in right:
        if name not in left:
            print(f"only right: {name}", file=sys.stderr)
    sys.exit(1)
PY
