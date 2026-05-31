#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT/apps/puffer-desktop"
TAURI_DIR="$APP_DIR/src-tauri"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ROOT/release}"
CACHE_DIR="${PUFFER_RELEASE_CACHE:-$ROOT/.release}"
RELEASE_TAG="${RELEASE_TAG:-ct}"
CEF_RELEASE_TAG="${CEF_RELEASE_TAG:-$RELEASE_TAG}"
GITHUB_REPO="${GITHUB_REPO:-berabuddies/puffer}"
CHROMIUM_TINTIN_DIR="${CHROMIUM_TINTIN_DIR:-$HOME/chromium_tintin}"
CHROMIUM_TINTIN_REPO="${CHROMIUM_TINTIN_REPO:-git@github.com:agentenv/chromium_tintin.git}"
CEF_REPO="${CEF_REPO:-https://github.com/chromiumembedded/cef.git}"
CEF_BRANCH="${CEF_BRANCH:-}"
LINUX_HOST="${LINUX_HOST:-c@65.19.161.135}"
LINUX_REPO_DIR="${LINUX_REPO_DIR:-/mnt/lvm_data/puffer}"
LINUX_CHROMIUM_TINTIN_DIR="${LINUX_CHROMIUM_TINTIN_DIR:-/mnt/lvm_data/chromium_tintin}"
NO_UPLOAD="${NO_UPLOAD:-0}"

log() {
  printf '==> %s\n' "$*" >&2
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: make <target>

Targets:
  build-rust         Build release puffer CLI/TUI binary.
  build-tauri        Build the platform Tauri app; macOS downloads CEF first.
  build-macos        macOS-only Rust + Tauri build. Hard fails off macOS.
  build-release-cef  Clone/pull/build CEF and upload puffer-cef-* to GitHub.
  pack-macos         Build and upload macOS .app zip plus TUI tarball.
  build-linux        Linux-only Rust + Tauri build. Hard fails off Linux.
  pack-linux         SSH-build Linux artifacts on c@65.19.161.135 and upload.
  pack-linux-local   Linux-only local package step used by pack-linux.

Common env:
  RELEASE_TAG=ct
  GITHUB_REPO=berabuddies/puffer
  CHROMIUM_TINTIN_DIR=$HOME/chromium_tintin
  CEF_REPO=https://github.com/chromiumembedded/cef.git
  CEF_BRANCH=<chromium-build-number>
  LINUX_HOST=c@65.19.161.135
  LINUX_REPO_DIR=/mnt/lvm_data/puffer
  LINUX_CHROMIUM_TINTIN_DIR=/mnt/lvm_data/chromium_tintin
  NO_UPLOAD=1
EOF
}

host_os() {
  uname -s | tr '[:upper:]' '[:lower:]'
}

host_arch() {
  case "$(uname -m)" in
    arm64 | aarch64) printf 'arm64' ;;
    x86_64 | amd64) printf 'x64' ;;
    *) uname -m ;;
  esac
}

release_platform() {
  case "$(host_os)" in
    darwin) printf 'macos' ;;
    linux) printf 'linux' ;;
    *) host_os ;;
  esac
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

require_macos() {
  [[ "$(host_os)" == "darwin" ]] || fail "$1 must run on macOS"
}

require_linux() {
  [[ "$(host_os)" == "linux" ]] || fail "$1 must run on Linux"
}

ensure_dirs() {
  mkdir -p "$ARTIFACT_DIR" "$CACHE_DIR"
}

reset_dir() {
  local dir="$1"
  case "$dir" in
    "$CACHE_DIR"/* | "$ARTIFACT_DIR"/*)
      rm -rf "$dir"
      mkdir -p "$dir"
      ;;
    *)
      fail "refusing to reset directory outside release cache: $dir"
      ;;
  esac
}

asset_arch() {
  host_arch
}

asset_platform() {
  release_platform
}

cef_asset_name() {
  printf 'puffer-cef-%s-%s.tar.gz' "$(asset_platform)" "$(asset_arch)"
}

desktop_asset_name() {
  local platform
  platform="$(asset_platform)"
  case "$platform" in
    macos) printf 'puffer-desktop-%s-%s.zip' "$platform" "$(asset_arch)" ;;
    *) printf 'puffer-desktop-%s-%s.tar.gz' "$platform" "$(asset_arch)" ;;
  esac
}

tui_asset_name() {
  printf 'puffer-tui-%s-%s.tar.gz' "$(asset_platform)" "$(asset_arch)"
}

ensure_release() {
  [[ "$NO_UPLOAD" == "1" ]] && return
  require_command gh
  if gh release view "$RELEASE_TAG" -R "$GITHUB_REPO" >/dev/null 2>&1; then
    return
  fi
  log "creating GitHub release $RELEASE_TAG in $GITHUB_REPO"
  gh release create "$RELEASE_TAG" \
    -R "$GITHUB_REPO" \
    --title "$RELEASE_TAG" \
    --notes "Puffer release artifacts for $RELEASE_TAG."
}

upload_asset() {
  local tag="$1"
  local asset="$2"
  [[ -f "$asset" ]] || fail "asset not found: $asset"
  if [[ "$NO_UPLOAD" == "1" ]]; then
    log "NO_UPLOAD=1; leaving asset at $asset"
    return
  fi
  require_command gh
  if ! gh release view "$tag" -R "$GITHUB_REPO" >/dev/null 2>&1; then
    log "creating GitHub release $tag in $GITHUB_REPO"
    gh release create "$tag" \
      -R "$GITHUB_REPO" \
      --title "$tag" \
      --notes "Puffer release artifacts for $tag."
  fi
  log "uploading $(basename "$asset") to $GITHUB_REPO@$tag"
  gh release upload "$tag" -R "$GITHUB_REPO" "$asset" --clobber
}

add_root_candidates() {
  local root="$1"
  printf '%s\n' "$root"
  printf '%s\n' "$root/Release"
  printf '%s\n' "$root/Release_GN_arm64"
  printf '%s\n' "$root/Release_GN_x64"
  printf '%s\n' "$root/Linux"
  printf '%s\n' "$root/LinuxNoOzone"
}

cef_runtime_ok() {
  local root="$1"
  case "$(asset_platform)" in
    macos)
      [[ -f "$root/Chromium Embedded Framework.framework/Chromium Embedded Framework" ]] &&
        [[ -f "$root/cefsimple Helper.app/Contents/MacOS/cefsimple Helper" ]]
      ;;
    linux)
      [[ -f "$root/libcef.so" ]]
      ;;
    *)
      return 1
      ;;
  esac
}

find_local_cef_runtime() {
  local roots=()
  local key
  for key in PUFFER_CEF_PATH PUFFER_CEF_ROOT CEF_PATH; do
    if [[ -n "${!key:-}" ]]; then
      roots+=("${!key}")
    fi
  done

  case "$(asset_platform)" in
    macos)
      roots+=(
        "$CHROMIUM_TINTIN_DIR/src/out/Release_GN_arm64"
        "$CHROMIUM_TINTIN_DIR/src/out/Release"
      )
      ;;
    linux)
      roots+=(
        "$CHROMIUM_TINTIN_DIR/src/out/Linux"
        "$CHROMIUM_TINTIN_DIR/src/out/LinuxNoOzone"
        "$CHROMIUM_TINTIN_DIR/src/out/Release"
      )
      ;;
  esac

  if [[ -d "$CHROMIUM_TINTIN_DIR/output" ]]; then
    while IFS= read -r cef_dir; do
      roots+=("$cef_dir/Release")
    done < <(find "$CHROMIUM_TINTIN_DIR/output" -maxdepth 1 -type d -name 'cef_binary_*' | sort)
  fi

  local root candidate
  for root in "${roots[@]}"; do
    while IFS= read -r candidate; do
      if cef_runtime_ok "$candidate"; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done < <(add_root_candidates "$root")
  done
  return 1
}

download_cef_release_runtime() {
  local platform="$1"
  local arch="$2"
  local asset="puffer-cef-$platform-$arch.tar.gz"
  local download_dir="$CACHE_DIR/downloads/$CEF_RELEASE_TAG"
  local extract_dir="$CACHE_DIR/cef/$platform-$arch"
  mkdir -p "$download_dir"
  rm -f "$download_dir/$asset"
  require_command gh
  log "checking GitHub release $CEF_RELEASE_TAG for $asset"
  if ! gh release download "$CEF_RELEASE_TAG" \
    -R "$GITHUB_REPO" \
    --pattern "$asset" \
    --dir "$download_dir" \
    --clobber >/dev/null; then
    return 1
  fi
  [[ -f "$download_dir/$asset" ]] || return 1
  reset_dir "$extract_dir"
  tar -xzf "$download_dir/$asset" -C "$extract_dir"
  local runtime
  runtime="$(find "$extract_dir" -type d -name Release -print -quit)"
  [[ -n "$runtime" ]] || fail "downloaded CEF release did not contain a Release directory"
  cef_runtime_ok "$runtime" || fail "downloaded CEF runtime is incomplete: $runtime"
  printf '%s\n' "$runtime"
}

ensure_cef_runtime_for_tauri() {
  if [[ "$(asset_platform)" != "macos" ]]; then
    return 0
  fi
  local runtime=""
  if runtime="$(download_cef_release_runtime "$(asset_platform)" "$(asset_arch)" 2>/dev/null)"; then
    log "using downloaded CEF runtime: $runtime"
    printf '%s\n' "$runtime"
    return 0
  fi
  runtime="$(find_local_cef_runtime)" || fail "macOS CEF runtime not found; run make build-release-cef or set PUFFER_CEF_PATH"
  log "using local CEF runtime: $runtime"
  printf '%s\n' "$runtime"
}

stage_tauri_cef_runtime() {
  local runtime="$1"
  local link="$TAURI_DIR/target/puffer-cef-runtime"
  mkdir -p "$(dirname "$link")"
  rm -rf "$link"
  ln -s "$runtime" "$link"
  printf '%s\n' "$link"
}

ensure_node_deps() {
  if [[ -d "$APP_DIR/node_modules" ]]; then
    return
  fi
  require_command npm
  log "installing desktop node dependencies"
  (cd "$APP_DIR" && npm ci)
}

build_rust() {
  require_command cargo
  log "building release puffer CLI/TUI"
  (cd "$ROOT" && cargo build --release -p puffer-cli)
}

build_tauri() {
  require_command npm
  ensure_node_deps
  local platform
  platform="$(asset_platform)"
  case "$platform" in
    macos)
      local cef_runtime
      local staged_runtime
      cef_runtime="$(ensure_cef_runtime_for_tauri)"
      staged_runtime="$(stage_tauri_cef_runtime "$cef_runtime")"
      log "building macOS Tauri app with CEF runtime $cef_runtime"
      (cd "$APP_DIR" && PUFFER_CEF_PATH="$staged_runtime" PUFFER_CEF_ROOT="$staged_runtime" npm run tauri -- build --bundles app)
      ;;
    linux)
      log "building Linux Tauri app"
      (cd "$APP_DIR" && npm run tauri -- build --bundles deb,appimage)
      ;;
    *)
      fail "unsupported Tauri build platform: $platform"
      ;;
  esac
}

build_macos() {
  require_macos build-macos
  build_rust
  build_tauri
}

build_linux() {
  require_linux build-linux
  build_rust
  build_tauri
}

chromium_src_dir() {
  printf '%s/src\n' "$CHROMIUM_TINTIN_DIR"
}

ensure_chromium_checkout() {
  local src
  src="$(chromium_src_dir)"
  if [[ ! -d "$src/.git" ]]; then
    require_command git
    mkdir -p "$(dirname "$src")"
    log "cloning Chromium tintin checkout to $src"
    git clone "$CHROMIUM_TINTIN_REPO" "$src" >&2
  fi

  if [[ ! -d "$src/.git" ]]; then
    fail "Chromium tintin checkout is missing: $src"
  fi

  local branch=""
  branch="$(git -C "$src" branch --show-current 2>/dev/null || true)"
  if [[ -n "$branch" && -z "$(git -C "$src" status --porcelain)" ]]; then
    log "pulling Chromium tintin checkout branch $branch"
    git -C "$src" pull --ff-only >&2 || git -C "$src" fetch github >&2
  else
    log "Chromium tintin checkout is detached or dirty; fetching only to preserve local fork changes"
    git -C "$src" fetch github >&2 || true
  fi
  printf '%s\n' "$src"
}

cef_branch_for_chromium() {
  local src="$1"
  if [[ -n "$CEF_BRANCH" ]]; then
    printf '%s\n' "$CEF_BRANCH"
    return
  fi
  awk -F= '$1 == "BUILD" { print $2 }' "$src/chrome/VERSION"
}

ensure_cef_checkout() {
  local src="$1"
  local cef_dir="$src/cef"
  local branch
  branch="$(cef_branch_for_chromium "$src")"
  [[ -n "$branch" ]] || fail "could not infer CEF branch from $src/chrome/VERSION"
  require_command git

  if [[ ! -d "$cef_dir/.git" ]]; then
    log "cloning CEF branch $branch to $cef_dir"
    git clone --branch "$branch" --single-branch "$CEF_REPO" "$cef_dir" >&2
    return
  fi

  local current_branch=""
  current_branch="$(git -C "$cef_dir" branch --show-current 2>/dev/null || true)"
  if [[ "$current_branch" == "$branch" && -z "$(git -C "$cef_dir" status --porcelain)" ]]; then
    log "pulling CEF checkout branch $branch"
    git -C "$cef_dir" pull --ff-only >&2
    return
  fi

  log "CEF checkout is detached, dirty, or on $current_branch; fetching branch $branch only"
  git -C "$cef_dir" fetch origin "$branch" >&2 || true
}

autoninja_path() {
  local src
  src="$(chromium_src_dir)"
  if [[ -x "$src/third_party/depot_tools/autoninja" ]]; then
    printf '%s\n' "$src/third_party/depot_tools/autoninja"
    return
  fi
  if command -v autoninja >/dev/null 2>&1; then
    command -v autoninja
    return
  fi
  fail "autoninja was not found"
}

ensure_depot_tools_bootstrapped() {
  local src="$1"
  local depot_tools="$src/third_party/depot_tools"
  [[ -x "$depot_tools/autoninja" ]] || return
  if [[ -x "$depot_tools/python-bin/python3" && -f "$depot_tools/python3_bin_reldir.txt" ]]; then
    return
  fi
  [[ -x "$depot_tools/ensure_bootstrap" ]] || return
  log "bootstrapping Chromium depot_tools"
  (cd "$depot_tools" && ./ensure_bootstrap >&2)
}

default_cef_out_dir() {
  local src="$1"
  case "$(asset_platform)" in
    macos) printf '%s/out/Release_GN_arm64\n' "$src" ;;
    linux)
      local candidate
      for candidate in "$src/out/Linux" "$src/out/LinuxNoOzone" "$src/out/Release" "$src/out/Release_GN_x64"; do
        if [[ -d "$candidate" ]]; then
          printf '%s\n' "$candidate"
          return
        fi
      done
      printf '%s/out/Release\n' "$src"
      ;;
    *) fail "unsupported CEF build platform: $(asset_platform)" ;;
  esac
}

run_cef_build() {
  local src="$1"
  local out_dir="${CEF_OUT_DIR:-}"
  local ninja
  if [[ -z "$out_dir" ]]; then
    out_dir="$(default_cef_out_dir "$src")"
  fi
  ensure_depot_tools_bootstrapped "$src"
  ninja="$(autoninja_path)"
  ensure_cef_checkout "$src"
  [[ -x "$src/cef/cef_create_projects.sh" ]] || fail "CEF project generator missing at $src/cef/cef_create_projects.sh"
  log "generating CEF projects"
  (cd "$src" && ./cef/cef_create_projects.sh >&2)
  log "building CEF target(s) ${CEF_BUILD_TARGETS:-cefsimple} in $out_dir"
  (cd "$src" && "$ninja" -C "$out_dir" ${CEF_BUILD_TARGETS:-cefsimple} >&2)
  printf '%s\n' "$out_dir"
}

cef_root_for_runtime() {
  local runtime="$1"
  local current="$runtime"
  local packaged_root
  while [[ "$current" != "/" ]]; do
    if [[ -f "$current/cef/include/cef_app.h" && -d "$current/cef/libcef_dll" ]]; then
      if packaged_root="$(packaged_cef_root_for_src "$current")"; then
        printf '%s\n' "$packaged_root"
        return 0
      fi
      printf '%s\n' "$current/cef"
      return 0
    fi
    if [[ -f "$current/include/cef_app.h" && -d "$current/libcef_dll" ]]; then
      printf '%s\n' "$current"
      return 0
    fi
    current="$(dirname "$current")"
  done
  return 1
}

packaged_cef_root_for_src() {
  local src_root="$1"
  local output_dir
  output_dir="$(dirname "$src_root")/output"
  [[ -d "$output_dir" ]] || return 1
  local path
  while IFS= read -r path; do
    if [[ -f "$path/include/cef_config.h" && -d "$path/libcef_dll" ]]; then
      printf '%s\n' "$path"
      return 0
    fi
  done < <(find "$output_dir" -maxdepth 1 -type d -name 'cef_binary_*' | sort)
  return 1
}

copy_macos_cef_runtime() {
  local runtime="$1"
  local dest="$2"
  require_command ditto
  mkdir -p "$dest"
  ditto "$runtime/Chromium Embedded Framework.framework" "$dest/Chromium Embedded Framework.framework"
  local helper found=0
  for helper in "$runtime"/cefsimple\ Helper*.app; do
    [[ -d "$helper" ]] || continue
    found=1
    ditto "$helper" "$dest/$(basename "$helper")"
  done
  [[ "$found" == "1" ]] || fail "no CEF helper apps found in $runtime"
  ensure_macos_helper_links "$dest"
}

ensure_macos_helper_links() {
  local runtime="$1"
  local helper helper_dir name
  for helper in "$runtime"/cefsimple\ Helper*.app; do
    [[ -d "$helper" ]] || continue
    helper_dir="$helper/Contents/MacOS"
    mkdir -p "$helper_dir"
    for name in libEGL.dylib libGLESv2.dylib libvk_swiftshader.dylib vk_swiftshader_icd.json; do
      [[ -e "$runtime/Chromium Embedded Framework.framework/Libraries/$name" ]] || continue
      [[ -e "$helper_dir/$name" ]] && continue
      ln -s "../../../Chromium Embedded Framework.framework/Libraries/$name" "$helper_dir/$name"
    done
  done
}

copy_linux_cef_runtime() {
  local runtime="$1"
  local dest="$2"
  mkdir -p "$dest"
  local item
  for item in \
    libcef.so chrome-sandbox icudtl.dat snapshot_blob.bin v8_context_snapshot.bin \
    libEGL.so libGLESv2.so libvk_swiftshader.so vk_swiftshader_icd.json \
    cefsimple cefclient; do
    [[ -e "$runtime/$item" ]] && cp -a "$runtime/$item" "$dest/"
  done
  for item in "$runtime"/*.pak "$runtime"/locales "$runtime"/swiftshader; do
    [[ -e "$item" ]] && cp -a "$item" "$dest/"
  done
}

copy_cef_runtime() {
  case "$(asset_platform)" in
    macos) copy_macos_cef_runtime "$1" "$2" ;;
    linux) copy_linux_cef_runtime "$1" "$2" ;;
    *) fail "unsupported CEF package platform: $(asset_platform)" ;;
  esac
}

package_cef_release() {
  local runtime="$1"
  local cef_root
  cef_root="$(cef_root_for_runtime "$runtime")" || fail "CEF headers/libcef_dll not found for runtime $runtime"
  cef_runtime_ok "$runtime" || fail "CEF runtime is incomplete: $runtime"

  ensure_dirs
  local platform arch name stage asset
  platform="$(asset_platform)"
  arch="$(asset_arch)"
  name="puffer-cef-$platform-$arch"
  stage="$CACHE_DIR/stage/$name"
  asset="$ARTIFACT_DIR/$name.tar.gz"
  reset_dir "$stage"
  mkdir -p "$stage/Release" "$stage/cef"
  copy_cef_runtime "$runtime" "$stage/Release"
  cp -a "$cef_root/include" "$stage/cef/include"
  cp -a "$cef_root/libcef_dll" "$stage/cef/libcef_dll"
  printf '%s\n' "$runtime" > "$stage/CEF_RUNTIME_SOURCE.txt"
  tar -C "$(dirname "$stage")" -czf "$asset" "$(basename "$stage")"
  upload_asset "$CEF_RELEASE_TAG" "$asset"
}

build_release_cef() {
  ensure_dirs
  local src out_dir runtime
  src="$(ensure_chromium_checkout)"
  out_dir="$(run_cef_build "$src")"
  runtime="$out_dir"
  cef_runtime_ok "$runtime" || fail "built CEF runtime is incomplete: $runtime"
  package_cef_release "$runtime"
}

mac_app_bundle() {
  find "$TAURI_DIR/target/release/bundle/macos" -maxdepth 1 -name '*.app' -type d -print -quit
}

mac_app_executable() {
  local app="$1"
  local executable=""
  if [[ -f "$app/Contents/Info.plist" ]]; then
    executable="$(plutil -extract CFBundleExecutable raw "$app/Contents/Info.plist" 2>/dev/null || true)"
  fi
  if [[ -n "$executable" && -x "$app/Contents/MacOS/$executable" ]]; then
    printf '%s\n' "$app/Contents/MacOS/$executable"
    return 0
  fi
  find "$app/Contents/MacOS" -maxdepth 1 -type f -perm -111 ! -name puffer -print -quit
}

bundle_macos_app() {
  require_macos pack-macos
  require_command ditto
  build_macos
  local source_app app_name stage app cef_runtime executable asset tui_asset tui_stage
  source_app="$(mac_app_bundle)"
  [[ -n "$source_app" ]] || fail "Tauri macOS app bundle was not produced"
  app_name="$(basename "$source_app")"
  cef_runtime="$(ensure_cef_runtime_for_tauri)"
  stage="$CACHE_DIR/stage/macos-app"
  reset_dir "$stage"
  ditto "$source_app" "$stage/$app_name"
  app="$stage/$app_name"

  [[ -x "$ROOT/target/release/puffer" ]] || build_rust
  cp "$ROOT/target/release/puffer" "$app/Contents/MacOS/puffer"
  mkdir -p "$app/Contents/MacOS/resources"
  rsync -a "$ROOT/resources/" "$app/Contents/MacOS/resources/"
  mkdir -p "$app/Contents/Frameworks"
  copy_macos_cef_runtime "$cef_runtime" "$app/Contents/Frameworks"

  executable="$(mac_app_executable "$app")"
  if [[ -n "$executable" ]]; then
    install_name_tool -add_rpath "@executable_path/../Frameworks" "$executable" 2>/dev/null || true
  fi

  ensure_dirs
  asset="$ARTIFACT_DIR/$(desktop_asset_name)"
  (cd "$stage" && ditto -c -k --sequesterRsrc --keepParent "$app_name" "$asset")
  upload_asset "$RELEASE_TAG" "$asset"

  tui_stage="$CACHE_DIR/stage/puffer-tui-$(asset_platform)-$(asset_arch)"
  tui_asset="$ARTIFACT_DIR/$(tui_asset_name)"
  reset_dir "$tui_stage"
  cp "$ROOT/target/release/puffer" "$tui_stage/puffer"
  tar -C "$(dirname "$tui_stage")" -czf "$tui_asset" "$(basename "$tui_stage")"
  upload_asset "$RELEASE_TAG" "$tui_asset"
}

pack_linux_local() {
  require_linux pack-linux-local
  build_linux
  ensure_dirs
  local stage asset tui_stage tui_asset appimage deb
  stage="$CACHE_DIR/stage/puffer-desktop-linux-$(asset_arch)"
  asset="$ARTIFACT_DIR/$(desktop_asset_name)"
  reset_dir "$stage"
  mkdir -p "$stage/app" "$stage/resources"
  appimage="$(find "$TAURI_DIR/target/release/bundle/appimage" -maxdepth 1 -name '*.AppImage' -type f -print -quit 2>/dev/null || true)"
  deb="$(find "$TAURI_DIR/target/release/bundle/deb" -maxdepth 1 -name '*.deb' -type f -print -quit 2>/dev/null || true)"
  [[ -n "$appimage" || -n "$deb" ]] || fail "Linux Tauri bundle did not produce AppImage or deb"
  [[ -n "$appimage" ]] && cp "$appimage" "$stage/app/"
  [[ -n "$deb" ]] && cp "$deb" "$stage/app/"
  [[ -x "$ROOT/target/release/puffer" ]] || build_rust
  cp "$ROOT/target/release/puffer" "$stage/puffer"
  rsync -a "$ROOT/resources/" "$stage/resources/"
  tar -C "$(dirname "$stage")" -czf "$asset" "$(basename "$stage")"
  upload_asset "$RELEASE_TAG" "$asset"

  tui_stage="$CACHE_DIR/stage/puffer-tui-linux-$(asset_arch)"
  tui_asset="$ARTIFACT_DIR/$(tui_asset_name)"
  reset_dir "$tui_stage"
  cp "$ROOT/target/release/puffer" "$tui_stage/puffer"
  tar -C "$(dirname "$tui_stage")" -czf "$tui_asset" "$(basename "$tui_stage")"
  upload_asset "$RELEASE_TAG" "$tui_asset"
}

pack_linux_remote() {
  require_command git
  require_command ssh
  require_command rsync
  local branch remote_artifacts
  branch="$(git -C "$ROOT" branch --show-current)"
  [[ -n "$branch" ]] || fail "pack-linux requires a named git branch"
  log "pushing $branch so the Linux host can sync it"
  git -C "$ROOT" push -u origin "$branch"

  log "building Linux artifacts on $LINUX_HOST"
  ssh "$LINUX_HOST" "bash -s" <<EOF
set -Eeuo pipefail
if [ ! -d "$LINUX_REPO_DIR/.git" ]; then
  mkdir -p "$(dirname "$LINUX_REPO_DIR")"
  git clone "git@github.com:$GITHUB_REPO.git" "$LINUX_REPO_DIR" || git clone "https://github.com/$GITHUB_REPO.git" "$LINUX_REPO_DIR"
fi
cd "$LINUX_REPO_DIR"
git fetch origin "$branch"
git checkout "$branch" || git checkout -b "$branch" "origin/$branch"
git pull --ff-only origin "$branch"
CHROMIUM_TINTIN_DIR="$LINUX_CHROMIUM_TINTIN_DIR" RELEASE_TAG="$RELEASE_TAG" GITHUB_REPO="$GITHUB_REPO" NO_UPLOAD=1 make build-release-cef
CHROMIUM_TINTIN_DIR="$LINUX_CHROMIUM_TINTIN_DIR" RELEASE_TAG="$RELEASE_TAG" GITHUB_REPO="$GITHUB_REPO" NO_UPLOAD=1 make pack-linux-local
EOF

  remote_artifacts="$ARTIFACT_DIR/remote-linux"
  reset_dir "$remote_artifacts"
  rsync -av "$LINUX_HOST:$LINUX_REPO_DIR/release/" "$remote_artifacts/"
  local asset
  for asset in "$remote_artifacts"/puffer-cef-linux-*.tar.gz \
    "$remote_artifacts"/puffer-desktop-linux-*.tar.gz \
    "$remote_artifacts"/puffer-tui-linux-*.tar.gz; do
    [[ -f "$asset" ]] || continue
    upload_asset "$RELEASE_TAG" "$asset"
  done
}

case "${1:-help}" in
  help) usage ;;
  build-rust) build_rust ;;
  build-tauri) build_tauri ;;
  build-macos) build_macos ;;
  build-release-cef) build_release_cef ;;
  pack-macos) bundle_macos_app ;;
  build-linux) build_linux ;;
  pack-linux) pack_linux_remote ;;
  pack-linux-local) pack_linux_local ;;
  *) usage; fail "unknown release target: ${1:-}" ;;
esac
