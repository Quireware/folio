# Pdfium pin

Desktop tests load a dynamic `libpdfium.so`. That file is not vendored and is not the F-Droid artefact.

| Fact | Value |
| --- | --- |
| Product version | 148.0.7763.0 (`MAJOR=148`, `MINOR=0`, `BUILD=7763`, `PATCH=0`) |
| pdfium-render feature | `pdfium_7763` (crate 0.9.5). Do not enable `pdfium_latest` or `static`. |
| Branch the 7763 binaries were built from | `chromium/7763` |
| Packaging commit | `b524047d815e1e304cc936f39b364d00964135cc` in `bblanchon/pdfium-binaries` only. This is not a pdfium source commit. |

`chromium/7763` is a moving branch name. Do not check out the branch tip and call it this pin.

The immutable source commit is `ca8a943c247c208fd7a9cd21b4de049f22b93070`, the `pdfium_revision` in the Chromium `148.0.7763.0` DEPS (`https://pdfium.googlesource.com/pdfium.git`). `nix/pdfium.nix` checks that commit out with gclient `checkout_configuration=small`. V8 and Skia sources are present; the GN args below leave both unused. `depot_tools` is `55659c2bf434a4fb508799fa93550822c5309ed5`, the `depot_tools_revision` in that DEPS.

`is_component_build = false` makes the upstream `component("pdfium")` target a source set, so the build changes that one target to `shared_library("pdfium")` and keeps `FPDF_EXPORT` visible. Those are the linux-x64 7763 archive's shared-library edits (`patches/shared_library.patch` and the `FPDF_EXPORT` part of `patches/public_headers.patch` at packaging commit `b524047`). They are not extra GN args.

The linux-x64 Nix build produced `libpdfium.so` from that commit. Its sha256 is `d34d749efb4f5341a2a678af7fe01ea5952740708dedb045ccb28b9b3b0cd70f`, and `FPDF_InitLibrary` loads. A second `nix-build --check` of the same derivation matched that output. A linux `.so` hash is not the Phase 19 APK payload compare. Android ABIs are not built.

The F-Droid and Nix builds must compile that C++ with V8 and XFA off. Do not ship the prebuilt `.so`. Do not use MuPDF. Android ABIs for the eventual build are `arm64-v8a` and `x86_64`.

GN args from the linux-x64 7763 archive (`args.gn`):

```
clang_use_chrome_plugins = false
is_component_build = false
is_debug = false
pdf_enable_v8 = false
pdf_enable_xfa = false
pdf_is_standalone = true
pdf_use_partition_alloc = false
target_cpu = "x64"
target_os = "linux"
treat_warnings_as_errors = false
```

Android uses the same flags except `target_os = "android"` and `target_cpu` of `arm64` or `x64`.

Local desktop test library, not an input to Nix:

`FOLIO_PDFIUM_DIR` or `/tmp/folio-pdfium/lib` containing `libpdfium.so` from `pdfium-linux-x64.tgz` of release `chromium/7763`.
