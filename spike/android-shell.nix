# Nix guideline compliant 2026-09-09
{
  pkgs ? import <nixpkgs> {
    config.android_sdk.accept_license = true;
    config.allowUnfree = true;
  },
}:
let
  ndkVersion = "27.2.12479018";
  android = pkgs.androidenv.composeAndroidPackages {
    cmdLineToolsVersion = "20.0";
    platformToolsVersion = "37.0.0";
    buildToolsVersions = [ "34.0.0" ];
    platformVersions = [ "34" ];
    includeCmake = false;
    includeNDK = true;
    ndkVersions = [ ndkVersion ];
    includeEmulator = true;
    emulatorVersion = "36.5.11";
    includeSystemImages = true;
    systemImageTypes = [ "google_apis" ];
    abiVersions = [ "x86_64" ];
  };
  androidHome = "${android.androidsdk}/libexec/android-sdk";
  androidNdkHome = "${androidHome}/ndk/${ndkVersion}";
in
pkgs.mkShell {
  packages = [
    android.androidsdk
    pkgs.jdk21
  ];

  ANDROID_HOME = androidHome;
  ANDROID_SDK_ROOT = androidHome;
  ANDROID_NDK_HOME = androidNdkHome;
  ANDROID_NDK_ROOT = androidNdkHome;
  NDK_HOME = androidNdkHome;
  JAVA_HOME = pkgs.jdk21.home;
  LANG = "C.UTF-8";
  LC_ALL = "C.UTF-8";
  GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/34.0.0/aapt2";
}
