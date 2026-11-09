#!/usr/bin/env bash
# Fail when unrar is part of the default build, or of the F-Droid build with --fdroid.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fdroid=0
extra=()
for arg in "$@"; do
  if [[ "$arg" == "--fdroid" ]]; then
    fdroid=1
  else
    extra+=("$arg")
  fi
done

python3 - "$root" "$fdroid" ${extra[@]+"${extra[@]}"} <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
fdroid = sys.argv[2] == "1"
extra = [Path(arg) for arg in sys.argv[3:]]

def workspace_manifests():
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r"(?ms)^\[workspace\](.*?)(?:\n\[|\Z)", text)
    body = match.group(1) if match else ""
    members = re.search(r"(?ms)^members\s*=\s*\[(.*?)\]", body)
    found = []
    if members:
        for raw in re.findall(r'"([^"]+)"', members.group(1)):
            manifest = root / raw / "Cargo.toml"
            if manifest.is_file():
                found.append(manifest)
    workspace = root / "Cargo.toml"
    if workspace.is_file():
        found.append(workspace)
    return found

def section(text, heading):
    match = re.search(rf"(?ms)^\[{re.escape(heading)}\]\s*\n(.*?)(?:\n\[|\Z)", text)
    return match.group(1) if match else ""

def features(text):
    body = section(text, "features")
    found = {}
    for name, raw in re.findall(r"(?ms)^([A-Za-z0-9_-]+)\s*=\s*\[(.*?)\]", body):
        found[name] = re.findall(r'"([^"]+)"', raw)
    return found

def enables_unrar(feats, name, seen):
    if name in seen:
        return False
    seen.add(name)
    if "unrar" in name.lower():
        return True
    for item in feats.get(name, []):
        token = item.removeprefix("dep:")
        if "unrar" in token.lower() or enables_unrar(feats, token, seen):
            return True
    return False

def required_unrar(text):
    for match in re.finditer(r"(?ms)^\[([^\]]+)\]\s*\n(.*?)(?:\n\[|\Z)", text):
        heading, body = match.group(1), match.group(2)
        if "dependencies" not in heading:
            continue
        for dep in re.finditer(r"(?ms)^(unrar)\s*=\s*(\{[^{}]*\}|\"[^\"]*\")", body):
            value = dep.group(2)
            if not re.search(r"optional\s*=\s*true", value):
                return True
    return False

def offending(text, fdroid_mode):
    if required_unrar(text):
        return True
    feats = features(text)
    names = ["default"]
    if fdroid_mode:
        names.extend(
            name
            for name in feats
            if name.lower() in {"fdroid", "f-droid", "fdroid-default"}
        )
    return any(enables_unrar(feats, name, set()) for name in names)

status = 0
manifests = workspace_manifests()
for path in extra:
    if path not in manifests:
        manifests.append(path)
if not manifests:
    print("licence-audit: no Cargo.toml files found", file=sys.stderr)
    sys.exit(1)
for path in manifests:
    if not path.is_file():
        print(f"licence-audit: missing {path}", file=sys.stderr)
        status = 1
        continue
    text = path.read_text(encoding="utf-8")
    if offending(text, fdroid):
        print(
            f"licence-audit: unrar is in the default or F-Droid build of {path}",
            file=sys.stderr,
        )
        status = 1

if fdroid:
    metadata = root / "metadata" / "dev.folio.reader.yml"
    if not metadata.is_file():
        print("licence-audit: missing metadata/dev.folio.reader.yml", file=sys.stderr)
        status = 1
    else:
        text = metadata.read_text(encoding="utf-8")
        if "--features cbr" in text or "--features=cbr" in text:
            print("licence-audit: F-Droid metadata enables cbr", file=sys.stderr)
            status = 1
sys.exit(status)
PY
