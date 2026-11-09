#!/usr/bin/env bash
set -euo pipefail
pkg-config --exists webkit2gtk-4.1
pkg-config --exists gtk+-3.0
pkg-config --exists libxdo
echo "desktop deps: ok"
