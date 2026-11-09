#!/usr/bin/env bash
# Compile the shared linux-x64 libpdfium.so from the synced tree.
# Google's clang and gn are Debian ELF binaries; point their interpreter at Nix glibc.
set -euo pipefail

linker=${PDFIUM_LINKER:?}
gcc_lib=${PDFIUM_GCC_LIB:?}
zlib_lib=${PDFIUM_ZLIB_LIB:?}
ncurses_lib=${PDFIUM_NCURSES_LIB:?}
glibc_lib=${PDFIUM_GLIBC_LIB:?}

export LD_LIBRARY_PATH=${gcc_lib}:${zlib_lib}:${ncurses_lib}:${glibc_lib}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
# Nix glibc 2.42 headers redirect strtol to __isoc23_*. The Debian sysroot
# libc does not provide those symbols, so the compile must not see Nix headers.
unset C_INCLUDE_PATH CPATH CPLUS_INCLUDE_PATH LIBRARY_PATH
export SOURCE_DATE_EPOCH=1
export PYTHONDONTWRITEBYTECODE=1
export LC_ALL=C

patch_interpreters() {
  local root=$1
  local path
  [[ -d $root ]] || return 0
  while IFS= read -r -d '' path; do
    if patchelf --print-interpreter "$path" >/dev/null 2>&1; then
      patchelf --set-interpreter "$linker" "$path"
    fi
  done < <(find "$root" -type f -print0)
}

patch_interpreters third_party/llvm-build/Release+Asserts/bin
patch_interpreters buildtools/linux64

# The sandbox has no /lib64 loader. Host tools (nasm) and libpdfium.so must
# use the Nix dynamic linker or the kernel cannot exec them.
wrap_clang_driver() {
  local bin driver bash_bin
  bin=third_party/llvm-build/Release+Asserts/bin/clang
  driver=$(readlink -f "$bin")
  if [[ ! -f $driver || -L $driver ]]; then
    echo "pdfium-build: clang driver is not a regular file: $driver" >&2
    exit 1
  fi
  mv "$driver" "$driver.real"
  bash_bin=$(command -v bash)
  if [[ $bash_bin != /* ]]; then
    echo "pdfium-build: bash is not an absolute path: ${bash_bin:-missing}" >&2
    exit 1
  fi
  {
    printf '#!%s\n' "$bash_bin"
    cat <<EOF
set -euo pipefail
link=1
for arg in "\$@"; do
  case \$arg in
    -c|-E|-S|-M|-MM|-fsyntax-only|--version|-dumpmachine|-dumpversion|--help|-print-*) link=0 ;;
  esac
done
if [[ \$link -eq 1 ]]; then
  # -nostdlib++ drops the driver's -lm. Linux GN lists dl/pthread/rt only.
  exec '${driver}.real' "\$@" -Wl,--dynamic-linker=${linker} -lm
fi
exec '${driver}.real' "\$@"
EOF
  } > "$driver"
  chmod 0755 "$driver"
}
wrap_clang_driver

python3 - <<'PY'
from pathlib import Path

build = Path("BUILD.gn")
text = build.read_text()
old = 'component("pdfium") {'
new = 'shared_library("pdfium") {'
count = text.count(old)
if count != 1:
    raise SystemExit(f"expected one pdfium component target, found {count}")
build.write_text(text.replace(old, new, 1))

header = Path("public/fpdfview.h")
src = header.read_text()
start = src.find("#if defined(COMPONENT_BUILD)\n")
if start < 0:
    raise SystemExit("FPDF_EXPORT component guard not found")
# The guard's else branch empties FPDF_EXPORT outside component builds.
# A shared library built with is_component_build=false still needs the symbols.
guard_else = "\n#else\n#define FPDF_EXPORT\n#endif  // defined(COMPONENT_BUILD)\n"
end = src.find(guard_else, start)
if end < 0:
    raise SystemExit("FPDF_EXPORT component else not found")
header.write_text(src[:start] + src[start + len("#if defined(COMPONENT_BUILD)\n") : end] + src[end + len(guard_else) :])
PY

mkdir -p out
cat > out/args.gn <<'EOF'
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
EOF

gn=buildtools/linux64/gn
if [[ ! -x $gn ]]; then
  echo "pdfium-build: gn missing at $gn" >&2
  find buildtools -maxdepth 3 -type f -print >&2
  exit 1
fi
"$gn" gen out
ninja -C out pdfium

if [[ ! -f out/libpdfium.so ]]; then
  echo "pdfium-build: out/libpdfium.so missing" >&2
  find out -name 'libpdfium*' -print >&2
  exit 1
fi

mkdir -p "$out/lib" "$out/share/folio"
cp -a out/libpdfium.so "$out/lib/libpdfium.so"
cp -a out/args.gn "$out/share/folio/args.gn"
if [[ -f build/util/LASTCHANGE ]]; then
  cp -a build/util/LASTCHANGE "$out/share/folio/LASTCHANGE"
fi
