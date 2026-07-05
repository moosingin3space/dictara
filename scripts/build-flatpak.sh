#!/usr/bin/env bash
# Build the Linux Flatpak bundle:
#   1. Build the Tauri .deb (in the docker/ci-linux container when the host
#      lacks the webkit2gtk build deps, e.g. on immutable distros)
#   2. Wrap it with flatpak-builder using flatpak/app.dictara.Dictara.yml
#
# Output: dist-flatpak/dictara.flatpak
#
# Usage:
#   scripts/build-flatpak.sh              # full build
#   scripts/build-flatpak.sh --skip-deb   # reuse existing .deb (fast iteration)
#   scripts/build-flatpak.sh --devel      # build app.dictara.Dictara.Devel
#                                         # (what CI ships from main)
#
# Install and run:
#   flatpak install --user --reinstall dist-flatpak/dictara.flatpak
#   flatpak run app.dictara.Dictara

set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_DEB=false
DEVEL=false
for arg in "$@"; do
  case "$arg" in
    --skip-deb) SKIP_DEB=true ;;
    --devel) DEVEL=true ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

CONTAINER_IMAGE="${DICTARA_CI_IMAGE:-dictara-ci-linux}"
TAURI_BUILD_ARGS=(build --bundles deb
  --config src-tauri/tauri.prod.conf.json
  --config src-tauri/tauri.linux.conf.json)

build_deb() {
  if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    echo "==> Building .deb on the host"
    npm ci
    npx tauri "${TAURI_BUILD_ARGS[@]}"
  else
    echo "==> Host lacks webkit2gtk dev libs; building .deb in container ($CONTAINER_IMAGE)"
    if ! podman image exists "$CONTAINER_IMAGE"; then
      echo "==> Container image not found, building it from docker/ci-linux"
      podman build -t "$CONTAINER_IMAGE" docker/ci-linux
    fi
    podman run --rm --security-opt label=disable \
      -v dictara-rust-tools:/opt/rust \
      -v "$PWD":/work -w /work \
      -e CARGO_HOME=/opt/rust/cargo \
      -e RUSTUP_HOME=/opt/rust/rustup \
      -e PATH=/opt/rust/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
      -e CARGO_TARGET_DIR=/work/src-tauri/target-linux \
      "$CONTAINER_IMAGE" \
      bash -c 'test -x /opt/rust/cargo/bin/cargo || (curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal); npm ci && npx tauri '"${TAURI_BUILD_ARGS[*]}"
  fi
}

if [[ "$SKIP_DEB" != "true" ]]; then
  build_deb
fi

# One of the two candidate dirs usually doesn't exist; don't let pipefail
# turn find's complaint into a silent script death
DEB=$(find src-tauri/target-linux/release/bundle/deb src-tauri/target/release/bundle/deb \
  -name '*.deb' 2>/dev/null | head -n1 || true)
if [[ -z "$DEB" ]]; then
  echo "error: no .deb found; run without --skip-deb first" >&2
  exit 1
fi
echo "==> Using $DEB"
cp "$DEB" flatpak/dictara.deb

APP_ID=app.dictara.Dictara
MANIFEST=flatpak/app.dictara.Dictara.yml
BUNDLE=dist-flatpak/dictara.flatpak
if [[ "$DEVEL" == "true" ]]; then
  scripts/gen-devel-manifest.sh
  APP_ID=app.dictara.Dictara.Devel
  MANIFEST=flatpak/app.dictara.Dictara.Devel.yml
  BUNDLE=dist-flatpak/dictara-devel.flatpak
fi

echo "==> Building Flatpak ($APP_ID)"
# flatpak-builder --user resolves deps against user-level remotes only
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --install-deps-from=flathub --force-clean \
  --repo=dist-flatpak/repo dist-flatpak/build "$MANIFEST"

echo "==> Creating bundle"
mkdir -p dist-flatpak
flatpak build-bundle dist-flatpak/repo "$BUNDLE" "$APP_ID"

echo "==> Done: $BUNDLE"
echo "    flatpak install --user --reinstall $BUNDLE"
echo "    flatpak run $APP_ID"
