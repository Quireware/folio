#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -z "${ANDROID_HOME:-}" || -z "${ANDROID_NDK_HOME:-}" ]]; then
  echo "ANDROID_HOME and ANDROID_NDK_HOME are required (NDK 27.2.12479018)" >&2
  exit 1
fi
if ! command -v dx >/dev/null 2>&1; then
  echo "dx 0.7.10 is required" >&2
  exit 1
fi
# Host dx defaults to x86_64 on this machine, which will not install on a phone.
dx build --package folio-app --android --target aarch64-linux-android
project=target/dx/folio/debug/android/app
res="${project}/app/src/main/res"
if [[ ! -d "${res}" ]]; then
  echo "dx did not generate ${res}" >&2
  exit 1
fi
# dx writes its own launcher icons and has already assembled the APK by now,
# so the overlay is copied in and Gradle runs a second time.
cp -R assets/android/res/. "${res}/"
cp assets/android/network_security_config.xml "${res}/xml/network_security_config.xml"
(cd "${project}" && ./gradlew assembleDebug)
echo "APK: ${project}/app/build/outputs/apk/debug/app-debug.apk"
