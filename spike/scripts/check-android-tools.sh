#!/usr/bin/env bash
set -euo pipefail

: "${ANDROID_HOME:?ANDROID_HOME unset}"
: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME unset}"
: "${JAVA_HOME:?JAVA_HOME unset}"

test -x "$ANDROID_HOME/platform-tools/adb"
test -x "$ANDROID_HOME/emulator/emulator"
test -d "$ANDROID_HOME/platforms/android-34"
test -d "$ANDROID_HOME/system-images/android-34/google_apis/x86_64"
test -x "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android24-clang"
command -v sdkmanager >/dev/null
command -v avdmanager >/dev/null
rustup target list --installed | grep -qx 'x86_64-linux-android'

printf 'android tools: ok\nANDROID_HOME=%s\nANDROID_NDK_HOME=%s\nJAVA_HOME=%s\n' \
  "$ANDROID_HOME" "$ANDROID_NDK_HOME" "$JAVA_HOME"
