{ lib
, stdenv
, stdenvNoCC
, python3
, git
, cacert
, curl
, unzip
, fetchurl
, patchelf
, ninja
, pkg-config
, zlib
, ncurses
}:
let
  version = "148.0.7763.0";
  # Chromium tag 148.0.7763.0 DEPS pdfium_revision. Not the chromium/7763 branch tip.
  rev = "ca8a943c247c208fd7a9cd21b4de049f22b93070";
  # depot_tools_revision from that same Pdfium DEPS.
  depotRev = "55659c2bf434a4fb508799fa93550822c5309ed5";
  pythonEnv = python3.withPackages (ps: with ps; [
    httplib2
    six
    requests
    certifi
    pyparsing
  ]);
  cipdClient = fetchurl {
    url = "https://chrome-infra-packages.appspot.com/client?platform=linux-amd64&version=git_revision:b1f414539ac10cc67a0250890a38712cc06cf102";
    hash = "sha256-pCRbvXkOAMQ3VsHCAsIaPStfCk/mVf200QACnf6fIjo=";
  };
  src = stdenvNoCC.mkDerivation {
    pname = "pdfium-src";
    inherit version;

    nativeBuildInputs = [
      pythonEnv
      git
      cacert
      curl
      unzip
    ];

    PDFIUM_REV = rev;
    PDFIUM_DEPOT_REV = depotRev;
    CIPD_CLIENT = cipdClient;
    NIX_SSL_CERT_FILE = "${cacert}/etc/ssl/certs/ca-bundle.crt";

    dontUnpack = true;
    dontConfigure = true;
    dontBuild = true;
    dontFixup = true;

    installPhase = ''
      runHook preInstall
      bash ${./pdfium-sync.sh}
      runHook postInstall
    '';

    outputHashAlgo = "sha256";
    outputHashMode = "recursive";
    # NAR hash of the normalized checkout at ca8a943c. Update this when the
    # sync script changes the tree.
    outputHash = "sha256-f9EayA1qZSoRISdadMCHg+Ek0DRnPvIEBYPGCzF5mTw=";
  };
in
stdenv.mkDerivation {
  pname = "pdfium";
  inherit version src;

  nativeBuildInputs = [
    pythonEnv
    patchelf
    ninja
    unzip
    # The Nix wrapper replaces PKG_CONFIG_PATH and hides the Debian sysroot
    # that Pdfium's pkg-config.py selects.
    pkg-config.passthru.pkg-config
  ];

  PDFIUM_LINKER = stdenv.cc.bintools.dynamicLinker;
  PDFIUM_GCC_LIB = "${stdenv.cc.cc.lib}/lib";
  PDFIUM_ZLIB_LIB = "${zlib}/lib";
  PDFIUM_NCURSES_LIB = "${ncurses}/lib";
  PDFIUM_GLIBC_LIB = "${stdenv.cc.libc}/lib";

  dontConfigure = true;
  doCheck = false;

  buildPhase = ''
    runHook preBuild
    bash ${./pdfium-build.sh}
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    if [[ ! -f $out/lib/libpdfium.so ]]; then
      echo "pdfium: build did not install libpdfium.so" >&2
      exit 1
    fi
    printf '%s\n' ${rev} > $out/share/folio/pdfium-revision
    runHook postInstall
  '';

  passthru = {
    inherit src rev depotRev;
  };

  meta = {
    description = "Pdfium ${version} built from source (linux-x64 shared library)";
    license = lib.licenses.bsd3;
  };
}
