{
  description = "folio local reader";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      each = fn: nixpkgs.lib.genAttrs systems (system: fn (import nixpkgs { inherit system; }));
      eachAndroid =
        fn:
        nixpkgs.lib.genAttrs systems (
          system:
          fn (
            import nixpkgs {
              inherit system;
              config = {
                allowUnfree = true;
                android_sdk.accept_license = true;
              };
            }
          )
        );
      ndkVersion = "27.2.12479018";
      # Pdfium 148.0.7763.0. The branch name is not an immutable source pin.
      # The commit is Chromium's pdfium_revision for that tag. See docs/pdfium-gn-args.md.
      pdfiumVersion = "148.0.7763.0";
      pdfiumBranch = "chromium/7763";
      pdfiumRevision = "ca8a943c247c208fd7a9cd21b4de049f22b93070";
    in
    {
      packages = each (
        pkgs:
        let
          desktop = pkgs.rustPlatform.buildRustPackage {
            pname = "folio-desktop";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            meta = {
              description = "folio local reader";
              license = pkgs.lib.licenses.mit;
              mainProgram = "folio";
            };
          };
        in
        {
          folio-desktop = desktop;
          default = desktop;
          pdfium = pkgs.callPackage ./nix/pdfium.nix { };
          folio-apk = pkgs.stdenv.mkDerivation {
            pname = "folio-apk";
            version = "0.1.0";
            src = ./.;
            dontConfigure = true;
            dontBuild = true;
            installPhase = ''
              mkdir -p $out/bin $out/share/folio
              cp scripts/build-apk-debug.sh $out/bin/folio-apk
              chmod +x $out/bin/folio-apk
              printf '%s\n' ${ndkVersion} > $out/share/folio/ndk-version
              printf '%s\n' ${pdfiumVersion} > $out/share/folio/pdfium-version
              printf '%s\n' ${pdfiumBranch} > $out/share/folio/pdfium-branch
              printf '%s\n' ${pdfiumRevision} > $out/share/folio/pdfium-revision
            '';
            meta = {
              description = "folio debug APK build script (NDK ${ndkVersion})";
              license = pkgs.lib.licenses.mit;
            };
          };
        }
      );

      checks = each (pkgs: {
        licence-audit = pkgs.runCommand "folio-licence-audit" { } ''
          cp -a ${./.} work
          chmod -R u+w work
          bash work/scripts/licence-audit.sh
          bash work/scripts/licence-audit.sh --fdroid
          if bash work/scripts/licence-audit.sh work/scripts/fixtures/unrar-default/Cargo.toml; then
            echo "licence-audit: unrar fixture was accepted" >&2
            exit 1
          fi
          touch $out
        '';
        fmt = pkgs.runCommand "folio-fmt" { nativeBuildInputs = [ pkgs.cargo pkgs.rustfmt ]; } ''
          cp -a ${./.} work
          chmod -R u+w work
          cd work
          cargo fmt --all -- --check
          touch $out
        '';
        test = self.packages.${pkgs.stdenv.hostPlatform.system}.folio-desktop;
        clippy = self.packages.${pkgs.stdenv.hostPlatform.system}.folio-desktop.overrideAttrs (old: {
          nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.clippy ];
          buildPhase = ''
            runHook preBuild
            cargo clippy --workspace --all-targets -- -D warnings
          '';
          doCheck = false;
          installPhase = ''
            mkdir -p $out
          '';
        });
      });

      devShells = eachAndroid (
        pkgs:
        let
          android = pkgs.androidenv.composeAndroidPackages {
            cmdLineToolsVersion = "20.0";
            platformToolsVersion = "37.0.0";
            buildToolsVersions = [ "34.0.0" ];
            platformVersions = [ "34" ];
            includeCmake = false;
            includeNDK = true;
            ndkVersions = [ ndkVersion ];
            includeEmulator = false;
            includeSystemImages = false;
          };
          androidHome = "${android.androidsdk}/libexec/android-sdk";
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rustfmt
              pkgs.clippy
              pkgs.pkg-config
              pkgs.jdk21
            ];
          };
          android = pkgs.mkShell {
            packages = [
              android.androidsdk
              pkgs.jdk21
              pkgs.cargo
              pkgs.rustc
            ];
            ANDROID_HOME = androidHome;
            ANDROID_SDK_ROOT = androidHome;
            ANDROID_NDK_HOME = "${androidHome}/ndk/${ndkVersion}";
            ANDROID_NDK_ROOT = "${androidHome}/ndk/${ndkVersion}";
            JAVA_HOME = pkgs.jdk21.home;
            GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/34.0.0/aapt2";
          };
        }
      );
    };
}
