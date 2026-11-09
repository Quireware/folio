#!/usr/bin/env bash
# Checkout Pdfium and the DEPS it names. The tree is normalized so the
# fixed-output hash does not depend on git metadata or the wall clock.
set -euo pipefail

rev=${PDFIUM_REV:?}
depot_rev=${PDFIUM_DEPOT_REV:?}
cipd_client=${CIPD_CLIENT:?}

export HOME=${TMPDIR:-/build}/home
export VPYTHON_BYPASS="manually managed python not supported by chrome operations"
export DEPOT_TOOLS_UPDATE=0
export DEPOT_TOOLS_WIN_TOOLCHAIN=0
export GIT_TERMINAL_PROMPT=0
export PYTHONDONTWRITEBYTECODE=1
export LC_ALL=C
# git log pages by default. The sandbox has no less, and a pager must not change the tree.
export GIT_PAGER=cat
export PAGER=cat
mkdir -p "$HOME"
git config --global user.email folio@localhost
git config --global user.name folio
git config --global init.defaultBranch main
git config --global advice.detachedHead false
git config --global --add safe.directory '*'

if [[ -n ${NIX_SSL_CERT_FILE:-} ]]; then
  export SSL_CERT_FILE=$NIX_SSL_CERT_FILE
  export GIT_SSL_CAINFO=$NIX_SSL_CERT_FILE
  export CURL_CA_BUNDLE=$NIX_SSL_CERT_FILE
fi

# httplib2 in nixpkgs has no socks submodule. gclient imports it at startup
# and only assigns a method onto the class; the sync itself does not open a proxy.
sp=$(python3 -c 'import httplib2, os; print(os.path.dirname(httplib2.__file__))')
py_shim=${TMPDIR:-/build}/py
mkdir -p "$py_shim/httplib2"
cp -a "$sp/." "$py_shim/httplib2/"
chmod -R u+w "$py_shim"
printf '%s\n' 'class socksocket:' '    pass' > "$py_shim/httplib2/socks.py"
export PYTHONPATH=$py_shim${PYTHONPATH:+:$PYTHONPATH}

work=${TMPDIR:-/build}/pdfium-sync
depot=$work/depot_tools
mkdir -p "$work"
git init "$depot"
git -C "$depot" remote add origin https://chromium.googlesource.com/chromium/tools/depot_tools.git
git -C "$depot" fetch --depth 1 origin "$depot_rev"
git -C "$depot" checkout --detach FETCH_HEAD
got_depot=$(git -C "$depot" rev-parse HEAD)
if [[ $got_depot != "$depot_rev" ]]; then
  echo "pdfium-sync: depot_tools at $got_depot, want $depot_rev" >&2
  exit 1
fi
echo "pdfium-sync: depot_tools $got_depot"

install -m 0755 "$cipd_client" "$work/cipd"
export CUSTOM_CIPD_CLIENT=$work/cipd

# The Nix sandbox has no /usr/bin/env, so depot_tools' vpython3 and cipd
# shebangs cannot start. gclient calls both by name. Public gs:// objects
# are fetched over HTTPS; gsutil would also demand luci-auth.
bash_bin=$(command -v bash)
if [[ $bash_bin != /* ]]; then
  echo "pdfium-sync: bash is not an absolute path: ${bash_bin:-missing}" >&2
  exit 1
fi
mkdir -p "$work/bin" /tmp
{
  printf '#!%s\n' "$bash_bin"
  cat <<'EOF'
set -euo pipefail
if [[ ${1:-} == *gsutil.py ]]; then
  cmd=${2:-}
  url=${3:-}
  case $cmd in
    cp)
      dest=${4:?}
      case $url in
        gs://*) ;;
        *)
          echo "pdfium-sync: refusing gsutil cp of $url" >&2
          exit 1
          ;;
      esac
      https=https://storage.googleapis.com/${url#gs://}
      echo "pdfium-sync: fetch $url" >&2
      curl --fail --location --retry 5 --retry-delay 2 --retry-connrefused \
        --silent --show-error -o "$dest" "$https" &
      curl_pid=$!
      while kill -0 "$curl_pid" 2>/dev/null; do
        sleep 20
        if [[ -f $dest ]]; then
          echo "pdfium-sync: $(basename "$dest") $(stat -c %s "$dest") bytes" >&2
        fi
      done
      wait "$curl_pid"
      exit 0
      ;;
    stat)
      exit 0
      ;;
    *)
      echo "pdfium-sync: refusing gsutil $cmd" >&2
      exit 1
      ;;
  esac
fi
args=()
for arg in "$@"; do
  case $arg in
    -vpython*) ;;
    *) args+=("$arg") ;;
  esac
done
if [[ ${#args[@]} -eq 0 ]]; then
  echo "pdfium-sync: vpython3 called with no script" >&2
  exit 1
fi
exec python3 "${args[@]}"
EOF
} > "$work/bin/vpython3"
{
  printf '#!%s\n' "$bash_bin"
  printf 'exec %q "$@"\n' "$work/cipd"
} > "$work/bin/cipd"
chmod 0755 "$work/bin/vpython3" "$work/bin/cipd"
export PATH=$work/bin:$depot:$PATH

cat > "$work/.gclient" <<EOF
solutions = [
  {
    "name": "pdfium",
    "url": "https://pdfium.googlesource.com/pdfium.git",
    "deps_file": "DEPS",
    "managed": False,
    "custom_deps": {},
    "custom_vars": {
      "checkout_configuration": "small",
    },
  },
]
target_os = ["linux"]
EOF

cd "$work"
echo "pdfium-sync: gclient sync starting"
python3 "$depot/gclient.py" sync \
  --no-history \
  --shallow \
  --nohooks \
  --jobs 12 \
  --revision "pdfium@${rev}"

if [[ ! -d pdfium/.git ]]; then
  echo "pdfium-sync: pdfium checkout missing" >&2
  exit 1
fi
got=$(git -C pdfium rev-parse HEAD)
if [[ $got != "$rev" ]]; then
  echo "pdfium-sync: pdfium at $got, want $rev" >&2
  exit 1
fi
echo "pdfium-sync: pdfium $got"
git --no-pager -C pdfium log -1 --format='%H %cI %s'

commit_time=$(git --no-pager -C pdfium log -1 --format=%ct)
commit_year=$(date -u -d "@${commit_time}" +%Y)
mkdir -p pdfium/build/util
printf 'LASTCHANGE=%s\nLASTCHANGE_YEAR=%s\n' "$got" "$commit_year" > pdfium/build/util/LASTCHANGE
printf '%s\n' "$commit_time" > pdfium/build/util/LASTCHANGE.committime

find pdfium \( -name .git -o -name .gitmodules \) -print0 | xargs -0 -r rm -rf
find pdfium -type d -name __pycache__ -print0 | xargs -0 -r rm -rf
find pdfium -type f -name '*.pyc' -delete
find pdfium -exec touch -h -d @1 {} +

mkdir -p "$out"
shopt -s dotglob nullglob
mv pdfium/* "$out"/
echo "pdfium-sync: installed $(find "$out" -type f | wc -l) files"
