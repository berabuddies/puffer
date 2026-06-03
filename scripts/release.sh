#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT/apps/puffer-desktop"
TAURI_DIR="$APP_DIR/src-tauri"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ROOT/release}"
CACHE_DIR="${PUFFER_RELEASE_CACHE:-$ROOT/.release}"
RELEASE_TAG="${RELEASE_TAG:-0.0.1-alpha}"
CEF_RELEASE_TAG="${CEF_RELEASE_TAG:-ct}"
LEGACY_GITHUB_REPO="${GITHUB_REPO:-}"
SOURCE_GITHUB_REPO="${SOURCE_GITHUB_REPO:-${LEGACY_GITHUB_REPO:-berabuddies/puffer}}"
RELEASE_GITHUB_REPO="${RELEASE_GITHUB_REPO:-${LEGACY_GITHUB_REPO:-berabuddies/puffer}}"
CEF_GITHUB_REPO="${CEF_GITHUB_REPO:-berabuddies/ct}"
CHROME_RELEASE_TAG="${CHROME_RELEASE_TAG:-$CEF_RELEASE_TAG}"
CHROME_GITHUB_REPO="${CHROME_GITHUB_REPO:-$CEF_GITHUB_REPO}"
UPLOAD_TUI_ARTIFACTS="${UPLOAD_TUI_ARTIFACTS:-0}"
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
  build-release-chrome
                      Package and upload Chromium Chrome from chromium_tintin.
  pack-macos         Build and upload macOS .app zip.
  build-linux        Linux-only Rust + Tauri build. Hard fails off Linux.
  pack-linux         SSH-build Linux artifacts on c@65.19.161.135 and upload.
  pack-linux-local   Linux-only local package step used by pack-linux.

Common env:
  RELEASE_TAG=0.0.1-alpha
  CEF_RELEASE_TAG=ct
  SOURCE_GITHUB_REPO=berabuddies/puffer
  RELEASE_GITHUB_REPO=berabuddies/puffer
  CEF_GITHUB_REPO=berabuddies/ct
  CHROME_GITHUB_REPO=berabuddies/ct
  CHROME_RELEASE_TAG=$CEF_RELEASE_TAG
  UPLOAD_TUI_ARTIFACTS=0
  CHROMIUM_TINTIN_DIR=$HOME/chromium_tintin
  CHROME_APP_PATH=<path-to-Chromium.app>
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

chrome_asset_name() {
  local platform
  platform="$(asset_platform)"
  case "$platform" in
    macos) printf 'chromium-tintin-chrome-%s-%s.zip' "$platform" "$(asset_arch)" ;;
    *) printf 'chromium-tintin-chrome-%s-%s.tar.gz' "$platform" "$(asset_arch)" ;;
  esac
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
  if gh release view "$RELEASE_TAG" -R "$RELEASE_GITHUB_REPO" >/dev/null 2>&1; then
    return
  fi
  log "creating GitHub release $RELEASE_TAG in $RELEASE_GITHUB_REPO"
  gh release create "$RELEASE_TAG" \
    -R "$RELEASE_GITHUB_REPO" \
    --title "$RELEASE_TAG" \
    --notes "Puffer release artifacts for $RELEASE_TAG."
}

upload_asset_to_repo() {
  local repo="$1"
  local tag="$2"
  local asset="$3"
  [[ -f "$asset" ]] || fail "asset not found: $asset"
  if [[ "$NO_UPLOAD" == "1" ]]; then
    log "NO_UPLOAD=1; leaving asset at $asset"
    return
  fi
  require_command gh
  if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    log "creating GitHub release $tag in $repo"
    gh release create "$tag" \
      -R "$repo" \
      --title "$tag" \
      --notes "Puffer release artifacts for $tag."
  fi
  log "uploading $(basename "$asset") to $repo@$tag"
  gh release upload "$tag" -R "$repo" "$asset" --clobber
}

upload_asset() {
  local tag="$1"
  local asset="$2"
  upload_asset_to_repo "$RELEASE_GITHUB_REPO" "$tag" "$asset"
}

upload_cef_asset() {
  local asset="$1"
  upload_asset_to_repo "$CEF_GITHUB_REPO" "$CEF_RELEASE_TAG" "$asset"
}

upload_chrome_asset() {
  local asset="$1"
  upload_asset_to_repo "$CHROME_GITHUB_REPO" "$CHROME_RELEASE_TAG" "$asset"
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
    -R "$CEF_GITHUB_REPO" \
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
      local app
      app="$(mac_app_bundle)"
      [[ -n "$app" ]] || fail "Tauri macOS app bundle was not produced"
      embed_macos_app_runtime "$app" "$cef_runtime"
      ;;
    linux)
      local bundles
      bundles="${LINUX_TAURI_BUNDLES:-deb}"
      log "building Linux Tauri app bundle(s): $bundles"
      (cd "$APP_DIR" && npm run tauri -- build --bundles "$bundles")
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
      for candidate in "$src/out/Linux" "$src/out/LinuxNoOzone" "$src/out/Release_GN_x64" "$src/out/Release"; do
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
  local depot_tools_path="$src/third_party/depot_tools:$PATH"
  ensure_depot_tools_bootstrapped "$src"
  ensure_cef_checkout "$src"
  [[ -x "$src/cef/cef_create_projects.sh" ]] || fail "CEF project generator missing at $src/cef/cef_create_projects.sh"
  log "generating CEF projects"
  (cd "$src/cef" && PATH="$depot_tools_path" ./cef_create_projects.sh >&2)
  apply_cef_compatibility_patches "$src"
  if [[ -z "$out_dir" ]]; then
    out_dir="$(default_cef_out_dir "$src")"
  fi
  ensure_cef_required_gn_args "$out_dir"
  regenerate_cef_gn "$src" "$out_dir" "$depot_tools_path"
  ninja="$(autoninja_path)"
  log "building CEF target(s) ${CEF_BUILD_TARGETS:-cefsimple} in $out_dir"
  (cd "$src" && PATH="$depot_tools_path" "$ninja" -C "$out_dir" ${CEF_BUILD_TARGETS:-cefsimple} >&2)
  printf '%s\n' "$out_dir"
}

set_gn_arg() {
  local args_file="$1"
  local key="$2"
  local value="$3"
  local tmp
  tmp="$(mktemp)"
  awk -v key="$key" -v value="$value" '
    BEGIN { done = 0 }
    $1 == key && $2 == "=" {
      print key " = " value
      done = 1
      next
    }
    { print }
    END {
      if (!done) {
        print key " = " value
      }
    }
  ' "$args_file" > "$tmp"
  mv "$tmp" "$args_file"
}

ensure_cef_required_gn_args() {
  local out_dir="$1"
  [[ "$(asset_platform)" == "linux" ]] || return
  local args_file="$out_dir/args.gn"
  [[ -f "$args_file" ]] || return
  set_gn_arg "$args_file" enable_widevine true
  set_gn_arg "$args_file" clang_use_chrome_plugins false
  set_gn_arg "$args_file" blink_heap_inside_shared_library true
}

regenerate_cef_gn() {
  local src="$1"
  local out_dir="$2"
  local depot_tools_path="$3"
  local rel_out="$out_dir"
  case "$out_dir" in
    "$src"/*) rel_out="${out_dir#"$src"/}" ;;
  esac
  log "regenerating GN files for CEF in $rel_out"
  (cd "$src" && PATH="$depot_tools_path" gn gen "$rel_out" >&2)
}

apply_cef_compatibility_patches() {
  local src="$1"
  local common_child_id="$src/content/public/common/child_process_id.h"
  local browser_child_id="$src/content/public/browser/child_process_id.h"

  if [[ ! -f "$common_child_id" && -f "$browser_child_id" ]]; then
    log "adding CEF compatibility shim for content/public/common/child_process_id.h"
    mkdir -p "$(dirname "$common_child_id")"
    cat > "$common_child_id" <<'EOF'
// Generated by Puffer's release build for CEF branches that still include the
// pre-Chromium-146 ChildProcessId path.
#ifndef CONTENT_PUBLIC_COMMON_CHILD_PROCESS_ID_H_
#define CONTENT_PUBLIC_COMMON_CHILD_PROCESS_ID_H_

#include "content/public/browser/child_process_id.h"

#endif  // CONTENT_PUBLIC_COMMON_CHILD_PROCESS_ID_H_
EOF
  fi

  local ax_collapse="$src/cef/libcef/renderer/accessibility/walk_ax_nodes_with_collapse.inc"
  local old_ax_parse='serialized_ids.Contains(StringToInt(child_id).value_or(0))'

  if [[ -f "$ax_collapse" ]] && grep -Fq "$old_ax_parse" "$ax_collapse"; then
    log "patching CEF accessibility node id parsing for current Blink String API"
    perl -0pi -e 's#      if \(serialized_ids\.Contains\(StringToInt\(child_id\)\.value_or\(0\)\)\) \{\n        filtered->emplace_back\(child_id\);\n      \}#      bool child_id_ok = false;\n      int parsed_child_id = child_id.ToInt(&child_id_ok);\n      if (child_id_ok && serialized_ids.Contains(parsed_child_id)) {\n        filtered->emplace_back(child_id);\n      }#' "$ax_collapse"
    if grep -Fq "$old_ax_parse" "$ax_collapse"; then
      fail "failed to patch CEF accessibility StringToInt compatibility"
    fi
  fi

  local ui_base_build="$src/ui/base/BUILD.gn"
  if [[ -f "$ui_base_build" ]] &&
    grep -Fq 'IS_OZONE_X11=$ozone_platform_x11' "$ui_base_build" &&
    ! grep -Fq 'SUPPORTS_OZONE_X11=' "$ui_base_build" &&
    grep -RIl 'BUILDFLAG(SUPPORTS_OZONE_X11)' "$src/cef" >/dev/null 2>&1; then
    log "patching CEF Ozone X11 buildflag name for current Chromium"
    while IFS= read -r cef_source; do
      perl -0pi -e 's#BUILDFLAG\(SUPPORTS_OZONE_X11\)#BUILDFLAG(IS_OZONE_X11)#g' "$cef_source"
    done < <(grep -RIl 'BUILDFLAG(SUPPORTS_OZONE_X11)' "$src/cef")
    if grep -RIl 'BUILDFLAG(SUPPORTS_OZONE_X11)' "$src/cef" >/dev/null 2>&1; then
      fail "failed to patch CEF Ozone X11 buildflag compatibility"
    fi
  fi

  local cef_thread_impl="$src/cef/libcef/common/thread_impl.cc"
  local platform_thread_h="$src/base/threading/platform_thread.h"
  if [[ -f "$cef_thread_impl" && -f "$platform_thread_h" ]] &&
    grep -Fq 'base::ThreadType::kPresentation' "$cef_thread_impl" &&
    ! grep -Fq 'kPresentation' "$platform_thread_h" &&
    grep -Fq 'kDisplayCritical' "$platform_thread_h"; then
    log "patching CEF thread priority for current Chromium ThreadType"
    perl -0pi -e 's#base::ThreadType::kPresentation#base::ThreadType::kDisplayCritical#g' "$cef_thread_impl"
    if grep -Fq 'base::ThreadType::kPresentation' "$cef_thread_impl"; then
      fail "failed to patch CEF ThreadType compatibility"
    fi
  fi

  local chrome_main_delegate_h="$src/chrome/app/chrome_main_delegate.h"
  local chrome_main_delegate_cc="$src/chrome/app/chrome_main_delegate.cc"
  if [[ -f "$chrome_main_delegate_h" ]] &&
    ! grep -Fq 'ui/base/resource/resource_bundle.h' "$chrome_main_delegate_h"; then
    log "patching ChromeMainDelegate resource bundle header include for CEF"
    perl -0pi -e 's@(#include "content/public/app/content_main_delegate.h"\n)@$1#include "ui/base/resource/resource_bundle.h"\n@' "$chrome_main_delegate_h"
    if ! grep -Fq 'ui/base/resource/resource_bundle.h' "$chrome_main_delegate_h"; then
      fail "failed to patch ChromeMainDelegate resource bundle header include"
    fi
  fi

  if [[ -f "$chrome_main_delegate_h" ]] &&
    ! grep -Fq 'GetResourceBundleDelegate()' "$chrome_main_delegate_h"; then
    log "patching ChromeMainDelegate resource bundle delegate hook for CEF"
    perl -0pi -e 's#(  bool IsInitFeatureListEarly\(\) override;\n)#${1}\n  virtual ui::ResourceBundle::Delegate* GetResourceBundleDelegate() {\n    return nullptr;\n  }\n#' "$chrome_main_delegate_h"
    if ! grep -Fq 'GetResourceBundleDelegate()' "$chrome_main_delegate_h"; then
      fail "failed to patch ChromeMainDelegate resource bundle delegate hook"
    fi
  fi

  if [[ -f "$chrome_main_delegate_cc" ]] &&
    grep -Fq 'chrome_feature_list_creator, invoked_in_browser->is_running_test' "$chrome_main_delegate_cc"; then
    log "patching ChromeMainDelegate browser resource bundle delegate call"
    perl -0pi -e 's#std::string actual_locale = LoadLocalState\(\n      chrome_feature_list_creator, invoked_in_browser->is_running_test\);#std::string actual_locale = LoadLocalState(\n      chrome_feature_list_creator, GetResourceBundleDelegate(),\n      invoked_in_browser->is_running_test);#' "$chrome_main_delegate_cc"
    if grep -Fq 'chrome_feature_list_creator, invoked_in_browser->is_running_test' "$chrome_main_delegate_cc"; then
      fail "failed to patch ChromeMainDelegate browser resource bundle delegate call"
    fi
  fi

  if [[ -f "$chrome_main_delegate_cc" ]] &&
    grep -Fq 'locale, nullptr, ui::ResourceBundle::LOAD_COMMON_RESOURCES' "$chrome_main_delegate_cc"; then
    log "patching ChromeMainDelegate subprocess resource bundle delegate call"
    perl -0pi -e 's#locale, nullptr, ui::ResourceBundle::LOAD_COMMON_RESOURCES#locale, GetResourceBundleDelegate(),\n            ui::ResourceBundle::LOAD_COMMON_RESOURCES#' "$chrome_main_delegate_cc"
    if grep -Fq 'locale, nullptr, ui::ResourceBundle::LOAD_COMMON_RESOURCES' "$chrome_main_delegate_cc"; then
      fail "failed to patch ChromeMainDelegate subprocess resource bundle delegate call"
    fi
  fi

  patch_cef_context_menu_compatibility "$src"
  patch_cef_browser_widget_compatibility "$src"
  patch_cef_toolbar_view_compatibility "$src"
  patch_cef_browser_view_compatibility "$src"
  patch_cef_tab_helpers_compatibility "$src"
  patch_cef_browser_delegate_compatibility "$src"
  patch_cef_touch_selection_compatibility "$src"
  patch_cef_originating_process_compatibility "$src"
  patch_cef_permission_prompt_compatibility "$src"
  patch_cef_chrome_lifecycle_compatibility "$src"
  patch_cef_content_main_compatibility "$src"

  local setting_helper_cc="$src/cef/libcef/browser/setting_helper.cc"
  local content_settings_types="$src/components/content_settings/core/common/content_settings_types.mojom"
  if [[ -f "$setting_helper_cc" && -f "$content_settings_types" ]] &&
    grep -Fq 'TO_CEF_TYPE(PERSISTENT_STORAGE);' "$setting_helper_cc" &&
    ! grep -Fq 'PERSISTENT_STORAGE' "$content_settings_types" &&
    grep -Fq 'DURABLE_STORAGE' "$content_settings_types"; then
    log "patching CEF persistent storage content setting for current Chromium"
    perl -0pi -e 's#    TO_CEF_TYPE\(PERSISTENT_STORAGE\);#    case ContentSettingsType::DURABLE_STORAGE:\n      return CEF_CONTENT_SETTING_TYPE_PERSISTENT_STORAGE;#' "$setting_helper_cc"
    perl -0pi -e 's#    FROM_CEF_TYPE\(PERSISTENT_STORAGE\);#    case CEF_CONTENT_SETTING_TYPE_PERSISTENT_STORAGE:\n      return ContentSettingsType::DURABLE_STORAGE;#' "$setting_helper_cc"
    if grep -Fq 'TO_CEF_TYPE(PERSISTENT_STORAGE);' "$setting_helper_cc" ||
      grep -Fq 'FROM_CEF_TYPE(PERSISTENT_STORAGE);' "$setting_helper_cc"; then
      fail "failed to patch CEF persistent storage content setting compatibility"
    fi
  fi

  local content_client_h="$src/content/public/browser/content_browser_client.h"
  local create_window_hook='virtual void CreateWindowResult(RenderFrameHost* opener, bool success)'

  if [[ -f "$content_client_h" ]] && ! grep -Fq "$create_window_hook" "$content_client_h"; then
    log "patching CEF CreateWindowResult browser client hook"
    perl -0pi -e 's#(      bool opener_suppressed,\n      bool\* no_javascript_access\);\n)#${1}\n  // Called after CreateNewWindow finishes for embedders that track pending\n  // window creation state.\n  virtual void CreateWindowResult(RenderFrameHost* opener, bool success) {}\n#' "$content_client_h"
    if ! grep -Fq "$create_window_hook" "$content_client_h"; then
      fail "failed to patch CEF CreateWindowResult compatibility"
    fi
  fi

  local content_client_cc="$src/content/public/browser/content_browser_client.cc"
  if [[ -f "$content_client_h" ]] && grep -Fq 'virtual void ConfigureNetworkContextParams(' "$content_client_h"; then
    log "patching CEF ConfigureNetworkContextParams base return type"
    perl -0pi -e 's#virtual void ConfigureNetworkContextParams\(#virtual bool ConfigureNetworkContextParams\(#' "$content_client_h"
  fi
  if [[ -f "$content_client_cc" ]] && grep -Fq 'void ContentBrowserClient::ConfigureNetworkContextParams(' "$content_client_cc"; then
    perl -0pi -e 's#void ContentBrowserClient::ConfigureNetworkContextParams\(#bool ContentBrowserClient::ConfigureNetworkContextParams\(#; s#(  network_context_params->accept_language = "en-us,en";\n)(?!  return true;\n)#${1}  return true;\n#' "$content_client_cc"
  fi

  local chrome_client_h="$src/chrome/browser/chrome_content_browser_client.h"
  local chrome_client_cc="$src/chrome/browser/chrome_content_browser_client.cc"
  if [[ -f "$chrome_client_h" ]] && grep -Fq 'void ConfigureNetworkContextParams(' "$chrome_client_h"; then
    log "patching CEF ConfigureNetworkContextParams Chrome return type"
    perl -0pi -e 's#void ConfigureNetworkContextParams\(#bool ConfigureNetworkContextParams\(#' "$chrome_client_h"
  fi
  if [[ -f "$chrome_client_cc" ]] && grep -Fq 'void ChromeContentBrowserClient::ConfigureNetworkContextParams(' "$chrome_client_cc"; then
    perl -0pi -e 's#void ChromeContentBrowserClient::ConfigureNetworkContextParams\(#bool ChromeContentBrowserClient::ConfigureNetworkContextParams\(#; s#(    network_context_params->accept_language = GetApplicationLocale\(\);\n  \}\n)(?!\n  return true;\n)#${1}\n  return true;\n#' "$chrome_client_cc"
  fi

  local shell_client_h="$src/content/shell/browser/shell_content_browser_client.h"
  local shell_client_cc="$src/content/shell/browser/shell_content_browser_client.cc"
  if [[ -f "$shell_client_h" ]] && grep -Fq 'void ConfigureNetworkContextParams(' "$shell_client_h"; then
    perl -0pi -e 's#void ConfigureNetworkContextParams\(#bool ConfigureNetworkContextParams\(#' "$shell_client_h"
  fi
  if [[ -f "$shell_client_cc" ]] && grep -Fq 'void ShellContentBrowserClient::ConfigureNetworkContextParams(' "$shell_client_cc"; then
    perl -0pi -e 's#void ShellContentBrowserClient::ConfigureNetworkContextParams\(#bool ShellContentBrowserClient::ConfigureNetworkContextParams\(#; s#(  ConfigureNetworkContextParamsForShell\(context, network_context_params,\n                                        cert_verifier_creation_params\);\n)(?!  return true;\n)#${1}  return true;\n#' "$shell_client_cc"
  fi

  local headless_client_h="$src/headless/lib/browser/headless_content_browser_client.h"
  local headless_client_cc="$src/headless/lib/browser/headless_content_browser_client.cc"
  if [[ -f "$headless_client_h" ]] && grep -Fq 'void ConfigureNetworkContextParams(' "$headless_client_h"; then
    perl -0pi -e 's#void ConfigureNetworkContextParams\(#bool ConfigureNetworkContextParams\(#' "$headless_client_h"
  fi
  if [[ -f "$headless_client_cc" ]] && grep -Fq 'void HeadlessContentBrowserClient::ConfigureNetworkContextParams(' "$headless_client_cc"; then
    perl -0pi -e 's#void HeadlessContentBrowserClient::ConfigureNetworkContextParams\(#bool HeadlessContentBrowserClient::ConfigureNetworkContextParams\(#; s#(      cert_verifier_creation_params\);\n)(?!  return true;\n)#${1}  return true;\n#' "$headless_client_cc"
  fi
}

patch_cef_content_main_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF content main patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def insert_after(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, marker + addition, 1))


def insert_before(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, addition + marker, 1))


def replace_once(path, old, new, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if old not in text:
        raise SystemExit(f"failed to patch {desc}: pattern not found in {path}")
    write(path, text.replace(old, new, 1))


def replace_optional(path, old, new):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if old in text:
        write(path, text.replace(old, new, 1))


content_main_h = "content/public/app/content_main.h"
insert_after(
    content_main_h,
    """#elif !BUILDFLAG(IS_ANDROID)
  int argc = 0;
  raw_ptr<const char*> argv = nullptr;
#endif
""",
    """
#if BUILDFLAG(IS_POSIX) && !BUILDFLAG(IS_ANDROID)
  bool disable_signal_handlers = false;
#endif
""",
    "disable_signal_handlers",
    "ContentMainParams CEF signal-handler flag",
)
insert_after(
    content_main_h,
    """#elif !BUILDFLAG(IS_ANDROID)
    copy.argc = argc;
    copy.argv = argv;
#endif
""",
    """#if BUILDFLAG(IS_POSIX) && !BUILDFLAG(IS_ANDROID)
    copy.disable_signal_handlers = disable_signal_handlers;
#endif
""",
    "copy.disable_signal_handlers",
    "ContentMainParams CEF signal-handler copy",
)
insert_after(
    content_main_h,
    """CONTENT_EXPORT int RunContentProcess(ContentMainParams params,
                                     ContentMainRunner* content_main_runner);
""",
    """
CONTENT_EXPORT int ContentMainInitialize(
    ContentMainParams params,
    ContentMainRunner* content_main_runner);
CONTENT_EXPORT int ContentMainRun(ContentMainRunner* content_main_runner);
CONTENT_EXPORT void ContentMainShutdown(ContentMainRunner* content_main_runner);
""",
    "ContentMainInitialize(",
    "content main split entrypoint declarations",
)

content_main_cc = "content/app/content_main.cc"
replace_once(
    content_main_cc,
    """// This function must be marked with NO_STACK_PROTECTOR or it may crash on
// return, see the --change-stack-guard-on-fork command line flag.
NO_STACK_PROTECTOR int RunContentProcess(
    ContentMainParams params,
    ContentMainRunner* content_main_runner) {
""",
    """int ContentMainInitialize(
    ContentMainParams params,
    ContentMainRunner* content_main_runner) {
""",
    "int ContentMainInitialize(",
    "content main initialize split",
)
replace_optional(
    content_main_cc,
    """  int exit_code = -1;
#if BUILDFLAG(IS_MAC)
  base::apple::ScopedNSAutoreleasePool autorelease_pool;
#endif

""",
    "  int exit_code = -1;\n",
)
replace_once(
    content_main_cc,
    "    SetupSignalHandlers();\n",
    """    if (!params.disable_signal_handlers) {
      SetupSignalHandlers();
    }
""",
    "params.disable_signal_handlers",
    "content main optional signal handlers",
)
replace_optional(
    content_main_cc,
    """#if BUILDFLAG(IS_MAC)
    // We need this pool for all the objects created before we get to the event
    // loop, but we don't want to leave them hanging around until the app quits.
    // Each "main" needs to flush this pool right before it goes into its main
    // event loop to get rid of the cruft. TODO(crbug.com/40260311): This
    // is not safe. Each main loop should create and destroy its own pool; it
    // should not be flushing the pool at the base of the autorelease pool
    // stack.
    params.autorelease_pool = &autorelease_pool;
    InitializeMac();
#endif
""",
    """#if BUILDFLAG(IS_MAC)
    InitializeMac();
#endif
""",
)
replace_once(
    content_main_cc,
    """  if (IsSubprocess())
    CommonSubprocessInit();
  exit_code = content_main_runner->Run();

#if !BUILDFLAG(IS_ANDROID) && !BUILDFLAG(IS_IOS)
  content_main_runner->Shutdown();
#endif

  return exit_code;
}
""",
    """  if (IsSubprocess()) {
    CommonSubprocessInit();
  }

  return exit_code;
}

// This function must be marked with NO_STACK_PROTECTOR or it may crash on
// return, see the --change-stack-guard-on-fork command line flag.
NO_STACK_PROTECTOR int ContentMainRun(
    ContentMainRunner* content_main_runner) {
  return content_main_runner->Run();
}

void ContentMainShutdown(ContentMainRunner* content_main_runner) {
#if !BUILDFLAG(IS_ANDROID) && !BUILDFLAG(IS_IOS)
  content_main_runner->Shutdown();
#endif
}

// This function must be marked with NO_STACK_PROTECTOR or it may crash on
// return, see the --change-stack-guard-on-fork command line flag.
NO_STACK_PROTECTOR int RunContentProcess(
    ContentMainParams params,
    ContentMainRunner* content_main_runner) {
#if BUILDFLAG(IS_MAC)
  base::apple::ScopedNSAutoreleasePool autorelease_pool;
  params.autorelease_pool = &autorelease_pool;
#endif

  int exit_code =
      ContentMainInitialize(std::move(params), content_main_runner);
  if (exit_code >= 0) {
    return exit_code;
  }

  exit_code = ContentMainRun(content_main_runner);
  ContentMainShutdown(content_main_runner);
  return exit_code;
}
""",
    "NO_STACK_PROTECTOR int ContentMainRun(",
    "content main run and shutdown split",
)

runner_h = "content/app/content_main_runner_impl.h"
insert_after(
    runner_h,
    "  void Shutdown() override;\n",
    "\n  void ShutdownOnUIThread();\n",
    "ShutdownOnUIThread()",
    "ContentMainRunnerImpl CEF UI-thread shutdown declaration",
)

runner_cc = "content/app/content_main_runner_impl.cc"
insert_after(
    runner_cc,
    """  delegate_ = nullptr;
  is_shutdown_ = true;
}
""",
    """
void ContentMainRunnerImpl::ShutdownOnUIThread() {
  discardable_shared_memory_manager_.reset();
  browser_memory_consumer_registry_.reset();
  memory_pressure_listener_registry_.reset();
}
""",
    "ContentMainRunnerImpl::ShutdownOnUIThread()",
    "ContentMainRunnerImpl CEF UI-thread shutdown definition",
)

chrome_main_h = "chrome/app/chrome_main_delegate.h"
insert_after(
    chrome_main_h,
    "  ~ChromeMainDelegate() override;\n",
    "\n  virtual void CleanupOnUIThread();\n",
    "virtual void CleanupOnUIThread();",
    "ChromeMainDelegate CEF cleanup declaration",
)

chrome_main_cc = "chrome/app/chrome_main_delegate.cc"
insert_before(
    chrome_main_cc,
    "std::optional<int> ChromeMainDelegate::PostEarlyInitialization(\n",
    """void ChromeMainDelegate::CleanupOnUIThread() {}

""",
    "ChromeMainDelegate::CleanupOnUIThread()",
    "ChromeMainDelegate CEF cleanup definition",
)
PY
}

patch_cef_chrome_lifecycle_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF Chrome lifecycle patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def insert_after(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, marker + addition, 1))


def insert_before(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, addition + marker, 1))


def replace_once(path, old, new, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if old not in text:
        return
    write(path, text.replace(old, new, 1))


chrome_client_h = "chrome/browser/chrome_content_browser_client.h"
insert_after(
    chrome_client_h,
    "  ~ChromeContentBrowserClient() override;\n",
    "\n  virtual void CleanupOnUIThread();\n",
    "virtual void CleanupOnUIThread();",
    "ChromeContentBrowserClient CEF cleanup hook declaration",
)

chrome_client_cc = "chrome/browser/chrome_content_browser_client.cc"
insert_after(
    chrome_client_cc,
    """ChromeContentBrowserClient::~ChromeContentBrowserClient() {
  // std::vector<> does not guarantee any specific destruction order, so
  // explicitly destroy elements in the reverse order per header comment.
  while (!extra_parts_.empty()) {
    extra_parts_.pop_back();
  }
}
""",
    """
void ChromeContentBrowserClient::CleanupOnUIThread() {
  keepalive_timer_.Stop();
}
""",
    "ChromeContentBrowserClient::CleanupOnUIThread()",
    "ChromeContentBrowserClient CEF cleanup hook definition",
)

content_client_h = "content/public/browser/content_browser_client.h"
insert_after(
    content_client_h,
    """  virtual bool HandleExternalProtocol(
      const GURL& url,
      base::RepeatingCallback<WebContents*()> web_contents_getter,
      FrameTreeNodeId frame_tree_node_id,
      NavigationUIData* navigation_data,
      bool is_primary_main_frame,
      bool is_in_fenced_frame_tree,
      network::mojom::WebSandboxFlags sandbox_flags,
      ui::PageTransition page_transition,
      bool has_user_gesture,
      const std::optional<url::Origin>& initiating_origin,
      RenderFrameHost* initiator_document,
      const net::IsolationInfo& isolation_info,
      mojo::PendingRemote<network::mojom::URLLoaderFactory>* out_factory);
""",
    """
  // Same as above, but exposes the whole request for embedders that need to
  // proxy or inspect external protocol navigations before Chrome handles them.
  virtual bool HandleExternalProtocol(
      base::RepeatingCallback<WebContents*()> web_contents_getter,
      FrameTreeNodeId frame_tree_node_id,
      NavigationUIData* navigation_data,
      bool is_primary_main_frame,
      bool is_in_fenced_frame_tree,
      network::mojom::WebSandboxFlags sandbox_flags,
      const network::ResourceRequest& request,
      const std::optional<url::Origin>& initiating_origin,
      RenderFrameHost* initiator_document,
      const net::IsolationInfo& isolation_info,
      mojo::PendingRemote<network::mojom::URLLoaderFactory>* out_factory) {
    return false;
  }
""",
    "Same as above, but exposes the whole request",
    "ContentBrowserClient CEF external-protocol overload",
)

nav_loader = "content/browser/loader/navigation_url_loader_impl.cc"
replace_once(
    nav_loader,
    "      resource_request.url, std::move(web_contents_getter),\n",
    "      resource_request.url, web_contents_getter,\n",
    "NavigationURLLoaderImpl reusable WebContents getter",
)
insert_after(
    nav_loader,
    """      request_info.isolation_info, &terminal_external_protocol);
""",
    """
  if (!handled) {
    handled = GetContentClient()->browser()->HandleExternalProtocol(
        web_contents_getter, frame_tree_node->frame_tree_node_id(),
        navigation_ui_data, request_info.is_primary_main_frame,
        frame_tree_node->IsInFencedFrameTree(), request_info.sandbox_flags,
        resource_request, initiating_origin,
        request_info.initiator_document_token
            ? RenderFrameHostImpl::FromDocumentToken(
                  request_info.initiator_process_id,
                  *request_info.initiator_document_token)
            : nullptr,
        request_info.isolation_info, &terminal_external_protocol);
  }
""",
    "resource_request, initiating_origin",
    "NavigationURLLoaderImpl CEF external-protocol request hook",
)

content_renderer_h = "content/public/renderer/content_renderer_client.h"
insert_after(
    content_renderer_h,
    "  virtual void RenderThreadStarted() {}\n",
    "\n  // Notifies that the RenderThread can now send sync IPC messages.\n"
    "  virtual void RenderThreadConnected() {}\n",
    "RenderThreadConnected()",
    "ContentRendererClient render-thread-connected hook",
)
insert_before(
    content_renderer_h,
    "  // Allows subclasses to enable some runtime features before Blink has\n",
    "  // Notifies that a DevTools agent has attached or detached.\n"
    "  virtual void DevToolsAgentAttached() {}\n"
    "  virtual void DevToolsAgentDetached() {}\n\n",
    "DevToolsAgentAttached()",
    "ContentRendererClient DevTools lifecycle hooks",
)

render_thread_impl = "content/renderer/render_thread_impl.cc"
insert_after(
    render_thread_impl,
    """  url_loader_throttle_provider_ =
      GetContentClient()->renderer()->CreateURLLoaderThrottleProvider(
          blink::URLLoaderThrottleProviderType::kFrame);
""",
    "  GetContentClient()->renderer()->RenderThreadConnected();\n",
    "RenderThreadConnected();",
    "RenderThreadImpl CEF connected notification",
)

blink_platform_h = "content/renderer/renderer_blink_platform_impl.h"
insert_after(
    blink_platform_h,
    "  void OnV8HeapLastResortGC() override;\n",
    "\n  void DevToolsAgentAttached() override;\n"
    "  void DevToolsAgentDetached() override;\n",
    "void DevToolsAgentAttached() override;",
    "RendererBlinkPlatformImpl DevTools hook declarations",
)

blink_platform_cc = "content/renderer/renderer_blink_platform_impl.cc"
insert_after(
    blink_platform_cc,
    """blink::mojom::PerformanceTier
RendererBlinkPlatformImpl::GetCpuPerformanceTier() {
  if (auto* render_thread = RenderThreadImpl::current()) {
    return render_thread->GetCpuPerformanceTier();
  }
  return blink::mojom::PerformanceTier::kUnknown;
}
""",
    """
void RendererBlinkPlatformImpl::DevToolsAgentAttached() {
  GetContentClient()->renderer()->DevToolsAgentAttached();
}

void RendererBlinkPlatformImpl::DevToolsAgentDetached() {
  GetContentClient()->renderer()->DevToolsAgentDetached();
}
""",
    "RendererBlinkPlatformImpl::DevToolsAgentAttached()",
    "RendererBlinkPlatformImpl DevTools hook definitions",
)
PY
}

patch_cef_permission_prompt_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF permission prompt patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def insert_before(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, addition + marker, 1))


def insert_after(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, marker + addition, 1))


permission_header = "chrome/browser/ui/permission_bubble/permission_prompt.h"
insert_before(
    permission_header,
    "// Factory function to create permission prompts for chrome.\n",
    """using CreatePermissionPromptFunctionPtr =
    std::unique_ptr<permissions::PermissionPrompt> (*)(
        content::WebContents* web_contents,
        permissions::PermissionPrompt::Delegate* delegate,
        bool* default_handling);
void SetCreatePermissionPromptFunction(CreatePermissionPromptFunctionPtr);

""",
    "CreatePermissionPromptFunctionPtr",
    "permission prompt CEF factory hook declaration",
)

factory_cc = "chrome/browser/ui/views/permissions/permission_prompt_factory.cc"
insert_before(
    factory_cc,
    "\n}  // namespace\n\nstd::unique_ptr<permissions::PermissionPrompt> CreatePermissionPrompt(\n",
    "\nCreatePermissionPromptFunctionPtr g_create_permission_prompt_ptr = nullptr;\n",
    "g_create_permission_prompt_ptr",
    "permission prompt CEF factory hook storage",
)
insert_after(
    factory_cc,
    "}  // namespace\n\n",
    """void SetCreatePermissionPromptFunction(
    CreatePermissionPromptFunctionPtr ptr) {
  g_create_permission_prompt_ptr = ptr;
}

""",
    "void SetCreatePermissionPromptFunction(",
    "permission prompt CEF factory hook setter",
)

factory_path = src / factory_cc
if factory_path.exists():
    text = read(factory_path)
    if "g_create_permission_prompt_ptr(web_contents, delegate," not in text:
        marker = (
            "std::unique_ptr<permissions::PermissionPrompt> CreatePermissionPrompt(\n"
            "    content::WebContents* web_contents,\n"
            "    permissions::PermissionPrompt::Delegate* delegate) {\n"
        )
        if marker not in text:
            raise SystemExit(
                "failed to patch permission prompt CEF factory hook call: "
                f"marker not found in {factory_path}"
            )
        hook = """  if (g_create_permission_prompt_ptr) {
    bool default_handling = true;
    auto prompt =
        g_create_permission_prompt_ptr(web_contents, delegate, &default_handling);
    if (prompt) {
      return prompt;
    }
    if (!default_handling) {
      return nullptr;
    }
  }

"""
        write(factory_path, text.replace(marker, marker + hook, 1))

delegate_header = src / "components/permissions/permission_prompt.h"
cef_prompt = src / "cef/libcef/browser/permission_prompt.cc"
if delegate_header.exists() and cef_prompt.exists():
    delegate_text = read(delegate_header)
    prompt_text = read(cef_prompt)
    has_zero_arg_decisions = "void SetPromptOptions(PromptOptions prompt_options)" in delegate_text
    uses_old_decision_calls = "delegate_->Accept(prompt_options)" in prompt_text

    if has_zero_arg_decisions and uses_old_decision_calls:
        prompt_text = prompt_text.replace(
            "    const PromptOptions prompt_options(std::monostate{});\n"
            "    switch (result) {\n",
            "    delegate_->SetPromptOptions(PromptOptions(std::monostate{}));\n"
            "    switch (result) {\n",
            1,
        )
        prompt_text = prompt_text.replace(
            "delegate_->Accept(prompt_options);", "delegate_->Accept();"
        )
        prompt_text = prompt_text.replace(
            "delegate_->Deny(prompt_options);", "delegate_->Deny();"
        )
        prompt_text = prompt_text.replace(
            "delegate_->Dismiss(prompt_options);", "delegate_->Dismiss();"
        )
        prompt_text = prompt_text.replace(
            "delegate_->Ignore(prompt_options);", "delegate_->Ignore();"
        )
        write(cef_prompt, prompt_text)
        prompt_text = read(cef_prompt)

    stale_calls = (
        "delegate_->Accept(prompt_options)",
        "delegate_->Deny(prompt_options)",
        "delegate_->Dismiss(prompt_options)",
        "delegate_->Ignore(prompt_options)",
    )
    if has_zero_arg_decisions and any(call in prompt_text for call in stale_calls):
        raise SystemExit(
            "failed to patch CEF permission prompt delegate decision compatibility"
        )
PY
}

patch_cef_browser_widget_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF browser widget patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def patch(path, old, new, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if old not in text:
        raise SystemExit(f"failed to patch {desc}: pattern not found in {path}")
    write(path, text.replace(old, new, 1))


def replace(path, old, new, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if old not in text:
        return
    write(path, text.replace(old, new, 1))


browser_widget_h = "chrome/browser/ui/views/frame/browser_widget.h"
replace(
    browser_widget_h,
    """ public:
  explicit BrowserWidget(BrowserView* browser_view);
""",
    """ public:
  BrowserWidget();
  explicit BrowserWidget(BrowserView* browser_view);
""",
    "BrowserWidget default constructor declaration",
)
replace(
    browser_widget_h,
    "  void UserChangedTheme(BrowserThemeChangeType theme_change_type);\n",
    "  virtual void UserChangedTheme(BrowserThemeChangeType theme_change_type);\n",
    "BrowserWidget virtual theme-change hook",
)
patch(
    browser_widget_h,
    """  void SetTabDragKind(TabDragKind tab_drag_kind);
  TabDragKind tab_drag_kind() const { return tab_drag_kind_; }

 protected:
""",
    """  void SetTabDragKind(TabDragKind tab_drag_kind);
  TabDragKind tab_drag_kind() const { return tab_drag_kind_; }

  BrowserView* browser_view() const { return browser_view_.get(); }

 protected:
""",
    "BrowserView* browser_view() const",
    "BrowserWidget browser_view accessor",
)
replace(
    browser_widget_h,
    """  // Callback for MenuRunner.
  void OnMenuClosed();

  // Select a native theme that is appropriate for the current context. This is
  // currently only needed for Linux to switch between the regular NativeTheme
  // and the GTK NativeTheme instance.
  void SelectNativeTheme();

  // Regenerate the frame on theme change if necessary. Returns true if
""",
    """  // Callback for MenuRunner.
  void OnMenuClosed();

  // Regenerate the frame on theme change if necessary. Returns true if
""",
    "BrowserWidget private SelectNativeTheme declaration",
)
patch(
    browser_widget_h,
    """ protected:
  // views::Widget:
""",
    """ protected:
  void SetBrowserFrameView(BrowserFrameView* browser_frame_view);
  void SetBrowserView(BrowserView* browser_view);

  // Select a native theme that is appropriate for the current context. This is
  // currently only needed for Linux to switch between the regular NativeTheme
  // and the GTK NativeTheme instance.
  void SelectNativeTheme();

  // views::Widget:
""",
    "void SetBrowserFrameView(BrowserFrameView* browser_frame_view);",
    "BrowserWidget CEF protected accessors",
)

browser_widget_cc = "chrome/browser/ui/views/frame/browser_widget.cc"
patch(
    browser_widget_cc,
    """////////////////////////////////////////////////////////////////////////////////
// BrowserWidget, public:

BrowserWidget::BrowserWidget(BrowserView* browser_view)
""",
    """////////////////////////////////////////////////////////////////////////////////
// BrowserWidget, public:

BrowserWidget::BrowserWidget() : BrowserWidget(nullptr) {}

BrowserWidget::BrowserWidget(BrowserView* browser_view)
""",
    "BrowserWidget::BrowserWidget() : BrowserWidget(nullptr) {}",
    "BrowserWidget default constructor definition",
)
patch(
    browser_widget_cc,
    """  set_focus_on_creation(false);
}

BrowserWidget::~BrowserWidget() {
""",
    """  set_focus_on_creation(false);
}

void BrowserWidget::SetBrowserFrameView(BrowserFrameView* browser_frame_view) {
  browser_frame_view_ = browser_frame_view;
}

void BrowserWidget::SetBrowserView(BrowserView* browser_view) {
  browser_view_ = browser_view;
}

BrowserWidget::~BrowserWidget() {
""",
    "BrowserWidget::SetBrowserFrameView",
    "BrowserWidget CEF setter definitions",
)
replace(
    browser_widget_cc,
    """  browser_view_->browser()->GetFeatures().TearDownPreBrowserWindowDestruction();
""",
    """  if (browser_view_ && browser_view_->browser()) {
    browser_view_->browser()->GetFeatures().TearDownPreBrowserWindowDestruction();
  }
""",
    "BrowserWidget CEF-safe destructor guard",
)
PY
}

patch_cef_toolbar_view_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF toolbar view patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def patch_toolbar_header():
    path = src / "chrome/browser/ui/views/toolbar/toolbar_view.h"
    if not path.exists():
        return
    text = read(path)
    if "#include <optional>" not in text:
        marker = "#include <memory>\n"
        if marker not in text:
            raise SystemExit(
                f"failed to patch ToolbarView optional include: marker not found in {path}"
            )
        text = text.replace(marker, marker + "#include <optional>\n", 1)
    sentinel = (
        "  ToolbarView(Browser* browser,\n"
        "              BrowserView* browser_view,\n"
        "              std::optional<DisplayMode> display_mode = std::nullopt);\n"
    )
    if sentinel not in text:
        old = "  ToolbarView(Browser* browser, BrowserView* browser_view);\n"
        if old not in text:
            raise SystemExit(
                f"failed to patch ToolbarView constructor declaration: pattern not found in {path}"
            )
        text = text.replace(old, sentinel, 1)
    write(path, text)


def patch_toolbar_definition():
    path = src / "chrome/browser/ui/views/toolbar/toolbar_view.cc"
    if not path.exists():
        return
    text = read(path)
    sentinel = (
        "ToolbarView::ToolbarView(Browser* browser,\n"
        "                         BrowserView* browser_view,\n"
        "                         std::optional<DisplayMode> display_mode)"
    )
    if sentinel not in text:
        old = "ToolbarView::ToolbarView(Browser* browser, BrowserView* browser_view)\n"
        if old not in text:
            raise SystemExit(
                f"failed to patch ToolbarView constructor definition: pattern not found in {path}"
            )
        text = text.replace(old, sentinel + "\n", 1)
    old_init = "      display_mode_(GetDisplayMode(browser)) {\n"
    new_init = "      display_mode_(display_mode.value_or(GetDisplayMode(browser))) {\n"
    if old_init in text:
        text = text.replace(old_init, new_init, 1)
    write(path, text)

    patched = read(path)
    if sentinel not in patched or new_init not in patched:
        raise SystemExit("failed to patch ToolbarView display-mode compatibility")


patch_toolbar_header()
patch_toolbar_definition()
PY
}

patch_cef_browser_view_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF browser view patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def replace_required(path, old, new, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"failed to patch {desc}: pattern not found in {path}")
    write(path, text.replace(old, new, 1))


def patch_browser_window_release():
    path = src / "chrome/browser/ui/browser.h"
    if not path.exists():
        return
    text = read(path)
    sentinel = "  void ReleaseBrowserWindow() { window_.release(); }\n"
    if sentinel in text:
        return
    marker = "  BrowserWindow* window() const { return window_.get(); }\n"
    if marker not in text:
        raise SystemExit(
            f"failed to patch Browser window release hook: marker not found in {path}"
        )
    addition = "\n  // Used when the BrowserWindow will outlive this Browser.\n" + sentinel
    write(path, text.replace(marker, marker + addition, 1))


def patch_browser_view_header():
    path = src / "chrome/browser/ui/views/frame/browser_view.h"
    if not path.exists():
        return
    text = read(path)
    if "  BrowserView();\n" not in text:
        old = "  explicit BrowserView(Browser* browser);\n"
        new = "  BrowserView();\n  explicit BrowserView(Browser* browser);\n"
        if old not in text:
            raise SystemExit(
                f"failed to patch BrowserView default constructor: pattern not found in {path}"
            )
        text = text.replace(old, new, 1)
    if "  void InitBrowser(Browser* browser);\n" not in text:
        old = "  explicit BrowserView(Browser* browser);\n"
        new = "  explicit BrowserView(Browser* browser);\n  void InitBrowser(Browser* browser);\n"
        if old not in text:
            raise SystemExit(
                f"failed to patch BrowserView InitBrowser declaration: pattern not found in {path}"
            )
        text = text.replace(old, new, 1)
    const_browser_member = "  const raw_ptr<Browser> browser_;\n"
    mutable_browser_member = "  raw_ptr<Browser> browser_;\n"
    if const_browser_member in text:
        text = text.replace(const_browser_member, mutable_browser_member, 1)
    hook = "  virtual ToolbarView* OverrideCreateToolbar() { return nullptr; }\n"
    if hook not in text:
        old = """ protected:
  // BrowserWindow:
  void DeleteBrowserWindow() final;

 private:
"""
        new = """  // Called during Toolbar destruction to remove dependent objects that have
  // dangling references.
  virtual void WillDestroyToolbar();

  // BrowserWindow:
  void DeleteBrowserWindow() final;

 protected:
  virtual ToolbarView* OverrideCreateToolbar() { return nullptr; }

 private:
"""
        if old not in text:
            raise SystemExit(
                f"failed to patch BrowserView CEF hook declarations: pattern not found in {path}"
            )
        text = text.replace(old, new, 1)
    write(path, text)


def patch_browser_view_source():
    path = src / "chrome/browser/ui/views/frame/browser_view.cc"
    if not path.exists():
        return
    text = read(path)
    if "void BrowserView::InitBrowser(Browser* browser)" not in text:
        old = """BrowserView::BrowserView(Browser* browser)
    : views::ClientView(nullptr, nullptr),
      exclusive_access_context_(
          std::make_unique<ExclusiveAccessContextImpl>(*this)),
      browser_(browser),
      accessibility_mode_observer_(
          std::make_unique<AccessibilityModeObserver>(this)) {
  SetShowIcon(::ShouldShowWindowIcon(
      browser_.get(), AppUsesWindowControlsOverlay(), AppUsesTabbed()));
"""
        new = """BrowserView::BrowserView() : BrowserView(nullptr) {}

BrowserView::BrowserView(Browser* browser)
    : views::ClientView(nullptr, nullptr),
      exclusive_access_context_(
          std::make_unique<ExclusiveAccessContextImpl>(*this)),
      accessibility_mode_observer_(
          std::make_unique<AccessibilityModeObserver>(this)) {
  if (browser) {
    InitBrowser(browser);
  }
}

void BrowserView::InitBrowser(Browser* browser) {
  DCHECK(!browser_);
  browser_ = browser;

  SetShowIcon(::ShouldShowWindowIcon(
      browser_.get(), AppUsesWindowControlsOverlay(), AppUsesTabbed()));
"""
        if old not in text:
            raise SystemExit(
                f"failed to patch BrowserView deferred initialization: pattern not found in {path}"
            )
        text = text.replace(old, new, 1)
    toolbar_old = """  toolbar_ = top_container_->AddChildView(
      std::make_unique<ToolbarView>(browser_.get(), this));
"""
    toolbar_new = """  toolbar_ = OverrideCreateToolbar();
  if (!toolbar_) {
    toolbar_ = new ToolbarView(browser_.get(), this);
  }
  top_container_->AddChildView(std::unique_ptr<ToolbarView>(toolbar_.get()));
"""
    if "  toolbar_ = OverrideCreateToolbar();\n" not in text:
        if toolbar_old not in text:
            raise SystemExit(
                f"failed to patch BrowserView toolbar creation: pattern not found in {path}"
            )
        text = text.replace(toolbar_old, toolbar_new, 1)
    destructor_marker = "BrowserView::~BrowserView() {\n"
    destructor_addition = (
        "  // If the Toolbar is overridden, detach it before the remaining\n"
        "  // BrowserView children are removed.\n"
        "  WillDestroyToolbar();\n\n"
    )
    if destructor_addition not in text:
        if destructor_marker not in text:
            raise SystemExit(
                f"failed to patch BrowserView toolbar teardown call: marker not found in {path}"
            )
        text = text.replace(
            destructor_marker, destructor_marker + destructor_addition, 1
        )
    will_destroy = """void BrowserView::WillDestroyToolbar() {
  autofill_bubble_handler_.reset();

  toolbar_button_provider_ = nullptr;
  if (toolbar_ && toolbar_->parent()) {
    toolbar_->parent()->RemoveChildView(toolbar_);
    toolbar_.ClearAndDelete();
  } else {
    toolbar_ = nullptr;
  }
}

"""
    if "void BrowserView::WillDestroyToolbar()" not in text:
        marker = "bool BrowserView::IsLoadingAnimationRunning() const {\n"
        if marker not in text:
            raise SystemExit(
                f"failed to patch BrowserView toolbar teardown hook: marker not found in {path}"
            )
        text = text.replace(marker, will_destroy + marker, 1)
    delete_old = """void BrowserView::DeleteBrowserWindow() {
  CHECK(browser_widget_);
"""
    delete_new = """void BrowserView::DeleteBrowserWindow() {
  if (!browser_widget_) {
    return;
  }
"""
    if delete_old in text:
        text = text.replace(delete_old, delete_new, 1)
    write(path, text)

    patched = read(path)
    required = (
        "void BrowserView::InitBrowser(Browser* browser)",
        "toolbar_ = OverrideCreateToolbar();",
        "void BrowserView::WillDestroyToolbar()",
        "if (!browser_widget_)",
    )
    missing = [item for item in required if item not in patched]
    if missing:
        raise SystemExit(
            "failed to patch BrowserView CEF compatibility: missing "
            + ", ".join(missing)
        )


patch_browser_window_release()
patch_browser_view_header()
patch_browser_view_source()
PY
}

patch_cef_tab_helpers_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF tab helpers patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])
path = src / "chrome/browser/ui/tab_helpers.h"
if not path.exists():
    raise SystemExit(f"failed to patch TabHelpers CEF access: missing {path}")

text = path.read_text(encoding="utf-8")
if '#include "cef/libcef/features/features.h"\n' not in text:
    marker = '#include "build/build_config.h"\n'
    if marker not in text:
        raise SystemExit(
            f"failed to patch TabHelpers CEF feature include: marker not found in {path}"
        )
    text = text.replace(
        marker, marker + '#include "cef/libcef/features/features.h"\n', 1
    )

forward = """#if BUILDFLAG(ENABLE_CEF)
class CefBrowserPlatformDelegateAlloy;
#endif

"""
if "class CefBrowserPlatformDelegateAlloy;" not in text:
    marker = """namespace tabs {
class TabModel;
}  // namespace tabs

"""
    if marker not in text:
        raise SystemExit(
            f"failed to patch TabHelpers CEF forward declaration: marker not found in {path}"
        )
    text = text.replace(marker, marker + forward, 1)

friend = """#if BUILDFLAG(ENABLE_CEF)
  friend class CefBrowserPlatformDelegateAlloy;
#endif

"""
if "friend class CefBrowserPlatformDelegateAlloy;" not in text:
    marker = "  friend class PreviewTab;\n\n"
    if marker not in text:
        raise SystemExit(
            f"failed to patch TabHelpers CEF friend access: marker not found in {path}"
        )
    text = text.replace(marker, marker + friend, 1)

path.write_text(text, encoding="utf-8")
PY
}

patch_cef_browser_delegate_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF browser delegate patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def patch_browser_header():
    path = src / "chrome/browser/ui/browser.h"
    if not path.exists():
        return
    text = read(path)
    if '#include "cef/libcef/features/features.h"\n' not in text:
        marker = '#include "build/build_config.h"\n'
        if marker not in text:
            raise SystemExit(
                f"failed to patch Browser CEF feature include: marker not found in {path}"
            )
        text = text.replace(
            marker, marker + '#include "cef/libcef/features/features.h"\n', 1
        )
    if '#include "cef/libcef/browser/chrome/browser_delegate.h"\n' not in text:
        marker = '#if BUILDFLAG(IS_ANDROID)\n'
        if marker not in text:
            raise SystemExit(
                f"failed to patch Browser CEF delegate include: marker not found in {path}"
            )
        include = """#if BUILDFLAG(ENABLE_CEF)
#include "cef/libcef/browser/chrome/browser_delegate.h"
#endif

"""
        text = text.replace(marker, include + marker, 1)
    if "scoped_refptr<cef::BrowserDelegate::CreateParams> cef_params;" not in text:
        marker = """    // Specifies the width for the uncollapsed Vertical Tab Strip.
    std::optional<int> vertical_tab_strip_uncollapsed_width;

   private:
"""
        addition = """    // Specifies the width for the uncollapsed Vertical Tab Strip.
    std::optional<int> vertical_tab_strip_uncollapsed_width;

#if BUILDFLAG(ENABLE_CEF)
    // Opaque CEF-specific configuration. Will be propagated to new Browsers.
    scoped_refptr<cef::BrowserDelegate::CreateParams> cef_params;

    // Specify the Browser that is opening this popup.
    // Currently only used with TYPE_PICTURE_IN_PICTURE and TYPE_DEVTOOLS.
    raw_ptr<BrowserWindowInterface, DanglingUntriaged> opener = nullptr;
#endif

   private:
"""
        if marker not in text:
            raise SystemExit(
                f"failed to patch Browser CreateParams CEF fields: marker not found in {path}"
            )
        text = text.replace(marker, addition, 1)
    if "cef::BrowserDelegate* cef_delegate() const" not in text:
        marker = """  BrowserWindowFeatures* browser_window_features() const {
    return features_.get();
  }

"""
        addition = marker + """#if BUILDFLAG(ENABLE_CEF)
  cef::BrowserDelegate* cef_delegate() const {
    return cef_browser_delegate_.get();
  }
#endif

"""
        if marker not in text:
            raise SystemExit(
                f"failed to patch Browser CEF delegate accessor: marker not found in {path}"
            )
        text = text.replace(marker, addition, 1)
    if "std::unique_ptr<cef::BrowserDelegate> cef_browser_delegate_;" not in text:
        marker = "  std::unique_ptr<TabStripModelDelegate> const tab_strip_model_delegate_;\n"
        addition = """#if BUILDFLAG(ENABLE_CEF)
  std::unique_ptr<cef::BrowserDelegate> cef_browser_delegate_;
#endif

"""
        if marker not in text:
            raise SystemExit(
                f"failed to patch Browser CEF delegate member: marker not found in {path}"
            )
        text = text.replace(marker, addition + marker, 1)
    write(path, text)


def patch_browser_source():
    path = src / "chrome/browser/ui/browser.cc"
    if not path.exists():
        return
    text = read(path)
    if "cef::BrowserDelegate::Create(this, params.cef_params, params.opener)" in text:
        return
    marker = """      type_(params.type),
      profile_(params.profile),
      window_(nullptr),
      tab_strip_model_delegate_(
"""
    addition = """      type_(params.type),
      profile_(params.profile),
      window_(nullptr),
#if BUILDFLAG(ENABLE_CEF)
      cef_browser_delegate_(
          cef::BrowserDelegate::Create(this, params.cef_params, params.opener)),
#endif
      tab_strip_model_delegate_(
"""
    if marker not in text:
        raise SystemExit(
            f"failed to patch Browser CEF delegate construction: marker not found in {path}"
        )
    write(path, text.replace(marker, addition, 1))


patch_browser_header()
patch_browser_source()
PY
}

patch_cef_touch_selection_compatibility() {
  local src="$1"
  local touch_controller_h="$src/ui/touch_selection/touch_selection_controller.h"
  local cef_touch_cc="$src/cef/libcef/browser/osr/touch_selection_controller_client_osr.cc"

  if [[ -f "$touch_controller_h" && -f "$cef_touch_cc" ]] &&
    grep -Fq 'ActiveStatus::kInactive' "$cef_touch_cc" &&
    ! grep -Fq 'kInactive' "$touch_controller_h" &&
    grep -Fq 'INACTIVE,' "$touch_controller_h"; then
    log "patching CEF touch selection active status for current Chromium"
    perl -0pi -e 's#ui::TouchSelectionController::ActiveStatus::kInactive#ui::TouchSelectionController::ActiveStatus::INACTIVE#g' "$cef_touch_cc"
    if grep -Fq 'ActiveStatus::kInactive' "$cef_touch_cc"; then
      fail "failed to patch CEF touch selection active status compatibility"
    fi
  fi
}

patch_cef_originating_process_compatibility() {
  local src="$1"
  local file
  for file in \
    "$src/cef/libcef/browser/net_service/browser_urlrequest_impl.cc" \
    "$src/cef/libcef/browser/net_service/resource_request_handler_wrapper.cc"; do
    if [[ ! -f "$file" ]]; then
      continue
    fi
    if grep -Fq 'services/network/public/cpp/originating_process.h' "$file"; then
      log "patching CEF originating process include for current Chromium in ${file#$src/}"
      perl -0pi -e 's#"services/network/public/cpp/originating_process.h"#"services/network/public/mojom/network_context.mojom.h"#g' "$file"
    fi
    if grep -Fq 'network::OriginatingProcess::browser()' "$file"; then
      log "patching CEF originating process constant for current Chromium in ${file#$src/}"
      perl -0pi -e 's#network::OriginatingProcess::browser\(\)#network::mojom::kBrowserProcessId#g' "$file"
    fi
    if grep -Fq 'services/network/public/cpp/originating_process.h' "$file" ||
      grep -Fq 'network::OriginatingProcess::browser()' "$file"; then
      fail "failed to patch CEF originating process compatibility in ${file#$src/}"
    fi
  done
}

patch_cef_context_menu_compatibility() {
  local src="$1"
  local python_bin
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif [[ -x "$src/third_party/depot_tools/python-bin/python3" ]]; then
    python_bin="$src/third_party/depot_tools/python-bin/python3"
  else
    fail "python3 was not found for CEF compatibility patching"
  fi

  "$python_bin" - "$src" <<'PY'
from pathlib import Path
import sys

src = Path(sys.argv[1])


def read(path):
    return path.read_text(encoding="utf-8")


def write(path, text):
    path.write_text(text, encoding="utf-8")


def replace_once(path, old, new, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"failed to patch {desc}: pattern not found in {path}")
    write(path, text.replace(old, new, 1))


def insert_before(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, addition + marker, 1))


def insert_after(path, marker, addition, sentinel, desc):
    path = src / path
    if not path.exists():
        return
    text = read(path)
    if sentinel in text:
        return
    if marker not in text:
        raise SystemExit(f"failed to patch {desc}: marker not found in {path}")
    write(path, text.replace(marker, marker + addition, 1))


render_menu_cc = "chrome/browser/renderer_context_menu/render_view_context_menu.cc"
insert_after(
    render_menu_cc,
    """base::OnceCallback<void(RenderViewContextMenu*)>* GetMenuShownCallback() {
  static base::NoDestructor<base::OnceCallback<void(RenderViewContextMenu*)>>
      callback;
  return callback.get();
}
""",
    """
RenderViewContextMenu::MenuCreatedCallback* GetMenuCreatedCallback() {
  static base::NoDestructor<RenderViewContextMenu::MenuCreatedCallback>
      callback;
  return callback.get();
}

RenderViewContextMenu::MenuShowHandlerCallback* GetMenuShowHandlerCallback() {
  static base::NoDestructor<RenderViewContextMenu::MenuShowHandlerCallback>
      callback;
  return callback.get();
}
""",
    "GetMenuCreatedCallback()",
    "RenderViewContextMenu CEF callback storage",
)
insert_before(
    render_menu_cc,
    "  observers_.AddObserver(&autofill_context_menu_manager_);\n",
    """  auto* cb = GetMenuCreatedCallback();
  if (!cb->is_null()) {
    first_observer_ = cb->Run(this);
    if (first_observer_) {
      observers_.AddObserver(first_observer_.get());
    }
  }

""",
    "first_observer_ = cb->Run(this);",
    "RenderViewContextMenu CEF menu-created observer",
)
insert_before(
    render_menu_cc,
    """}

Profile* RenderViewContextMenu::GetProfile() const {
""",
    """  if (first_observer_) {
    first_observer_->InitMenu(params_);
  }
""",
    "first_observer_->InitMenu(params_);",
    "RenderViewContextMenu CEF observer init hook",
)
insert_after(
    render_menu_cc,
    """void RenderViewContextMenu::RemoveObserverForTesting(
    RenderViewContextMenuObserver* observer) {
  observers_.RemoveObserver(observer);
}
""",
    """
// static
void RenderViewContextMenu::RegisterMenuCreatedCallback(
    MenuCreatedCallback cb) {
  *GetMenuCreatedCallback() = cb;
}

// static
void RenderViewContextMenu::RegisterMenuShowHandlerCallback(
    MenuShowHandlerCallback cb) {
  *GetMenuShowHandlerCallback() = cb;
}

bool RenderViewContextMenu::UseShowHandler() {
  auto* cb = GetMenuShowHandlerCallback();
  return !cb->is_null() && cb->Run(this);
}
""",
    "RegisterMenuCreatedCallback(",
    "RenderViewContextMenu CEF callback methods",
)

render_menu_h = "chrome/browser/renderer_context_menu/render_view_context_menu.h"
insert_before(
    render_menu_h,
    " protected:\n",
    """
  using MenuCreatedCallback = base::RepeatingCallback<
      std::unique_ptr<RenderViewContextMenuObserver>(RenderViewContextMenu*)>;
  static void RegisterMenuCreatedCallback(MenuCreatedCallback cb);

  using MenuShowHandlerCallback =
      base::RepeatingCallback<bool(RenderViewContextMenu*)>;
  static void RegisterMenuShowHandlerCallback(MenuShowHandlerCallback cb);

""",
    "RegisterMenuCreatedCallback(MenuCreatedCallback cb)",
    "RenderViewContextMenu CEF callback declarations",
)
insert_after(
    render_menu_h,
    " protected:\n",
    "  bool UseShowHandler();\n\n",
    "bool UseShowHandler();",
    "RenderViewContextMenu CEF show-handler declaration",
)
insert_before(
    render_menu_h,
    "  // An observer that handles spelling suggestions, \"Add to dictionary\", and\n",
    """  std::unique_ptr<RenderViewContextMenuObserver> first_observer_;

""",
    "first_observer_;",
    "RenderViewContextMenu CEF first observer storage",
)

views_menu_cc = "chrome/browser/ui/views/renderer_context_menu/render_view_context_menu_views.cc"
insert_after(
    views_menu_cc,
    """bool RenderViewContextMenuViews::GetAcceleratorForCommandId(
    int command_id,
    ui::Accelerator* accel) const {
""",
    """  if (RenderViewContextMenu::GetAcceleratorForCommandId(command_id, accel)) {
    return true;
  }

""",
    "RenderViewContextMenu::GetAcceleratorForCommandId(command_id, accel)",
    "RenderViewContextMenuViews CEF accelerator lookup",
)
insert_after(
    views_menu_cc,
    "void RenderViewContextMenuViews::Show() {\n",
    """  if (UseShowHandler()) {
    return;
  }

""",
    "UseShowHandler()",
    "RenderViewContextMenuViews CEF show handler",
)
insert_before(
    views_menu_cc,
    "\nviews::Widget* RenderViewContextMenuViews::GetTopLevelWidget() {\n",
    """
bool RenderViewContextMenuViews::IsRunning() {
  return static_cast<ToolkitDelegateViews*>(toolkit_delegate())
      ->IsMenuRunning();
}
""",
    "RenderViewContextMenuViews::IsRunning()",
    "RenderViewContextMenuViews CEF running query",
)
insert_after(
    "chrome/browser/ui/views/renderer_context_menu/render_view_context_menu_views.h",
    "  void Show() override;\n",
    "  bool IsRunning() override;\n",
    "bool IsRunning() override;",
    "RenderViewContextMenuViews CEF running declaration",
)

delegate_views_h = "chrome/browser/ui/views/tab_contents/chrome_web_contents_view_delegate_views.h"
delegate_views_cc = "chrome/browser/ui/views/tab_contents/chrome_web_contents_view_delegate_views.cc"
insert_after(
    delegate_views_h,
    "  void ShowMenu(std::unique_ptr<RenderViewContextMenuBase> menu) override;\n",
    "  bool IsMenuRunning() override;\n",
    "bool IsMenuRunning() override;",
    "ChromeWebContentsViewDelegateViews CEF running declaration",
)
insert_before(
    delegate_views_cc,
    "\nvoid ChromeWebContentsViewDelegateViews::ShowContextMenu(\n",
    """
bool ChromeWebContentsViewDelegateViews::IsMenuRunning() {
  return context_menu_ && context_menu_->IsRunning();
}
""",
    "ChromeWebContentsViewDelegateViews::IsMenuRunning()",
    "ChromeWebContentsViewDelegateViews CEF running query",
)

mac_delegate_h = "chrome/browser/ui/views/tab_contents/chrome_web_contents_view_delegate_views_mac.h"
mac_delegate_mm = "chrome/browser/ui/views/tab_contents/chrome_web_contents_view_delegate_views_mac.mm"
insert_after(
    mac_delegate_h,
    "  void ShowMenu(std::unique_ptr<RenderViewContextMenuBase> menu) override;\n",
    "  bool IsMenuRunning() override;\n",
    "bool IsMenuRunning() override;",
    "ChromeWebContentsViewDelegateViewsMac CEF running declaration",
)
insert_before(
    mac_delegate_mm,
    "\ncontent::RenderWidgetHostView*\nChromeWebContentsViewDelegateViewsMac::GetActiveRenderWidgetHostView() const {\n",
    """
bool ChromeWebContentsViewDelegateViewsMac::IsMenuRunning() {
  return context_menu_ && context_menu_->IsRunning();
}
""",
    "ChromeWebContentsViewDelegateViewsMac::IsMenuRunning()",
    "ChromeWebContentsViewDelegateViewsMac CEF running query",
)

context_delegate_h = "components/renderer_context_menu/context_menu_delegate.h"
insert_after(
    context_delegate_h,
    "  virtual void ShowMenu(std::unique_ptr<RenderViewContextMenuBase> menu) = 0;\n",
    "  virtual bool IsMenuRunning() = 0;\n",
    "virtual bool IsMenuRunning()",
    "ContextMenuDelegate CEF running API",
)

base_h = "components/renderer_context_menu/render_view_context_menu_base.h"
insert_after(
    base_h,
    "  void Cancel();\n",
    "\n  virtual bool IsRunning() = 0;\n",
    "virtual bool IsRunning()",
    "RenderViewContextMenuBase CEF running API",
)
insert_after(
    base_h,
    "  const content::ContextMenuParams& params() const { return params_; }\n",
    "  content::WebContents* source_web_contents() const { return source_web_contents_; }\n",
    "source_web_contents() const",
    "RenderViewContextMenuBase CEF web contents accessor",
)
insert_after(
    base_h,
    "  bool IsCommandIdChecked(int command_id) const override;\n",
    """  bool GetAcceleratorForCommandId(int command_id,
                                  ui::Accelerator* accelerator) const override;
""",
    "GetAcceleratorForCommandId(int command_id",
    "RenderViewContextMenuBase CEF accelerator declaration",
)
insert_before(
    "components/renderer_context_menu/render_view_context_menu_base.cc",
    "\nvoid RenderViewContextMenuBase::ExecuteCommand(int id, int event_flags) {\n",
    """
bool RenderViewContextMenuBase::GetAcceleratorForCommandId(
    int id,
    ui::Accelerator* accelerator) const {
  for (auto& observer : observers_) {
    if (observer.IsCommandIdSupported(id)) {
      return observer.GetAccelerator(id, accelerator);
    }
  }

  return false;
}
""",
    "RenderViewContextMenuBase::GetAcceleratorForCommandId",
    "RenderViewContextMenuBase CEF accelerator lookup",
)

observer_h = "components/renderer_context_menu/render_view_context_menu_observer.h"
insert_after(
    observer_h,
    """namespace content {
struct ContextMenuParams;
}
""",
    """
namespace ui {
class Accelerator;
}
""",
    "class Accelerator;",
    "RenderViewContextMenuObserver CEF accelerator forward declaration",
)
insert_after(
    observer_h,
    "  virtual bool IsCommandIdEnabled(int command_id);\n",
    "  virtual bool GetAccelerator(int command_id, ui::Accelerator* accel);\n",
    "virtual bool GetAccelerator(int command_id",
    "RenderViewContextMenuObserver CEF accelerator API",
)
insert_after(
    "components/renderer_context_menu/render_view_context_menu_observer.cc",
    """bool RenderViewContextMenuObserver::IsCommandIdEnabled(int command_id) {
  return false;
}
""",
    """
bool RenderViewContextMenuObserver::GetAccelerator(int command_id,
                                                   ui::Accelerator* accel) {
  return false;
}
""",
    "RenderViewContextMenuObserver::GetAccelerator",
    "RenderViewContextMenuObserver CEF accelerator default",
)

toolkit_h = "components/renderer_context_menu/views/toolkit_delegate_views.h"
toolkit_cc = "components/renderer_context_menu/views/toolkit_delegate_views.cc"
insert_after(
    toolkit_h,
    "  views::MenuItemView* menu_view() { return menu_view_; }\n",
    "  bool IsMenuRunning() const;\n",
    "bool IsMenuRunning() const;",
    "ToolkitDelegateViews CEF running declaration",
)
insert_before(
    toolkit_cc,
    "\nvoid ToolkitDelegateViews::Init(ui::SimpleMenuModel* menu_model) {\n",
    """
bool ToolkitDelegateViews::IsMenuRunning() const {
  return menu_runner_ && menu_runner_->IsRunning();
}
""",
    "ToolkitDelegateViews::IsMenuRunning()",
    "ToolkitDelegateViews CEF running query",
)

for rel in [
    "chrome/browser/ui/cocoa/renderer_context_menu/render_view_context_menu_mac_cocoa.h",
    "chrome/browser/ui/cocoa/renderer_context_menu/render_view_context_menu_mac_remote_cocoa.h",
]:
    insert_after(
        rel,
        "  void Show() override;\n",
        "  bool IsRunning() override;\n",
        "bool IsRunning() override;",
        "Mac RenderViewContextMenu CEF running declaration",
    )

insert_after(
    "chrome/browser/ui/cocoa/renderer_context_menu/render_view_context_menu_mac_cocoa.mm",
    "void RenderViewContextMenuMacCocoa::Show() {\n",
    """  if (UseShowHandler()) {
    return;
  }

""",
    "UseShowHandler()",
    "RenderViewContextMenuMacCocoa CEF show handler",
)
insert_before(
    "chrome/browser/ui/cocoa/renderer_context_menu/render_view_context_menu_mac_cocoa.mm",
    "\nvoid RenderViewContextMenuMacCocoa::CancelToolkitMenu() {\n",
    """
bool RenderViewContextMenuMacCocoa::IsRunning() {
  return menu_controller_ && [menu_controller_ isMenuOpen];
}
""",
    "RenderViewContextMenuMacCocoa::IsRunning()",
    "RenderViewContextMenuMacCocoa CEF running query",
)
insert_before(
    "chrome/browser/ui/cocoa/renderer_context_menu/render_view_context_menu_mac_remote_cocoa.mm",
    "\nvoid RenderViewContextMenuMacRemoteCocoa::CancelToolkitMenu() {\n",
    """
bool RenderViewContextMenuMacRemoteCocoa::IsRunning() {
  return runner_ && runner_->IsRunning();
}
""",
    "RenderViewContextMenuMacRemoteCocoa::IsRunning()",
    "RenderViewContextMenuMacRemoteCocoa CEF running query",
)
PY
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
      rm -f "$helper_dir/$name"
      cp -a "$runtime/Chromium Embedded Framework.framework/Libraries/$name" "$helper_dir/$name"
    done
  done
}

sign_macos_app_bundle() {
  local app="$1"
  local identity="${MACOS_CODESIGN_IDENTITY:--}"
  require_command codesign
  log "signing macOS app bundle with identity '$identity'"
  codesign --force --deep --sign "$identity" "$app" >&2
  codesign --verify --deep --strict --verbose=2 "$app" >&2
}

verify_macos_app_zip() {
  local asset="$1"
  local app_name="$2"
  require_command codesign
  require_command ditto

  local verify_dir verify_app
  verify_dir="$(mktemp -d "${TMPDIR:-/tmp}/puffer-macos-zip.XXXXXX")"
  if ! ditto -x -k "$asset" "$verify_dir"; then
    rm -rf "$verify_dir"
    fail "failed to extract macOS app zip: $asset"
  fi
  verify_app="$verify_dir/$app_name"
  if [[ ! -d "$verify_app" ]]; then
    rm -rf "$verify_dir"
    fail "macOS app zip did not contain $app_name"
  fi
  log "verifying signed macOS app zip $asset"
  if ! codesign --verify --deep --strict --verbose=2 "$verify_app" >&2; then
    rm -rf "$verify_dir"
    fail "macOS app zip signature verification failed: $asset"
  fi
  rm -rf "$verify_dir"
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
    if [[ -e "$runtime/$item" ]]; then
      cp -a "$runtime/$item" "$dest/"
    fi
  done
  for item in "$runtime"/*.pak "$runtime"/locales "$runtime"/swiftshader; do
    if [[ -e "$item" ]]; then
      cp -a "$item" "$dest/"
    fi
  done
}

copy_cef_runtime() {
  case "$(asset_platform)" in
    macos) copy_macos_cef_runtime "$1" "$2" ;;
    linux) copy_linux_cef_runtime "$1" "$2" ;;
    *) fail "unsupported CEF package platform: $(asset_platform)" ;;
  esac
}

chrome_app_ok() {
  local app="$1"
  [[ -d "$app" ]] || return 1
  [[ -f "$app/Contents/Info.plist" ]] || return 1
  [[ -d "$app/Contents/MacOS" ]] || return 1
}

find_local_chrome_app() {
  local apps=()
  local key
  for key in CHROME_APP_PATH CHROMIUM_APP_PATH; do
    if [[ -n "${!key:-}" ]]; then
      apps+=("${!key}")
    fi
  done
  apps+=(
    "$CHROMIUM_TINTIN_DIR/src/out/Release/Chromium.app"
    "$CHROMIUM_TINTIN_DIR/src/out/Release_GN_arm64/Chromium.app"
    "$CHROMIUM_TINTIN_DIR/src/out/Release_GN_x64/Chromium.app"
  )

  local app
  for app in "${apps[@]}"; do
    if chrome_app_ok "$app"; then
      printf '%s\n' "$app"
      return 0
    fi
  done
  return 1
}

linux_chrome_runtime_ok() {
  local runtime="$1"
  [[ -x "$runtime/chrome" ]] || return 1
  [[ -f "$runtime/icudtl.dat" ]] || return 1
  [[ -f "$runtime/resources.pak" ]] || return 1
  [[ -d "$runtime/locales" ]] || return 1
}

find_local_chrome_runtime() {
  local roots=()
  local key
  for key in CHROME_RUNTIME_PATH CHROMIUM_RUNTIME_PATH; do
    if [[ -n "${!key:-}" ]]; then
      roots+=("${!key}")
    fi
  done
  roots+=(
    "$CHROMIUM_TINTIN_DIR/src/out/Linux"
    "$CHROMIUM_TINTIN_DIR/src/out/LinuxNoOzone"
    "$CHROMIUM_TINTIN_DIR/src/out/Release_GN_x64"
    "$CHROMIUM_TINTIN_DIR/src/out/Release"
  )

  local root candidate
  for root in "${roots[@]}"; do
    while IFS= read -r candidate; do
      if linux_chrome_runtime_ok "$candidate"; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done < <(add_root_candidates "$root")
  done
  return 1
}

copy_linux_chrome_runtime() {
  local runtime="$1"
  local dest="$2"
  mkdir -p "$dest"

  local item
  for item in \
    "$runtime"/chrome "$runtime"/chrome-wrapper "$runtime"/chrome_crashpad_handler \
    "$runtime"/chrome-sandbox "$runtime"/chrome_sandbox \
    "$runtime"/icudtl.dat "$runtime"/snapshot_blob.bin \
    "$runtime"/v8_context_snapshot.bin "$runtime"/vk_swiftshader_icd.json \
    "$runtime"/*.pak "$runtime"/lib*.so "$runtime"/lib*.so.*; do
    [[ "$item" == *.TOC ]] && continue
    if [[ -e "$item" ]]; then
      cp -a "$item" "$dest/"
    fi
  done

  for item in \
    "$runtime"/locales "$runtime"/resources "$runtime"/MEIPreload \
    "$runtime"/PrivacySandboxAttestationsPreloaded \
    "$runtime"/IwaKeyDistribution "$runtime"/hyphen-data "$runtime"/angledata; do
    if [[ -e "$item" ]]; then
      cp -a "$item" "$dest/"
    fi
  done
}

package_macos_chrome_release() {
  require_macos build-release-chrome
  require_command ditto
  ensure_dirs

  local source_app stage app asset
  source_app="$(find_local_chrome_app)" || fail "Chromium.app not found; set CHROME_APP_PATH or CHROMIUM_TINTIN_DIR"
  log "packaging Chromium Chrome app from $source_app"
  stage="$CACHE_DIR/stage/chromium-tintin-chrome-$(asset_platform)-$(asset_arch)"
  reset_dir "$stage"
  ditto "$source_app" "$stage/Chromium.app"
  app="$stage/Chromium.app"
  sign_macos_app_bundle "$app"

  asset="$ARTIFACT_DIR/$(chrome_asset_name)"
  (cd "$stage" && ditto -c -k --sequesterRsrc --keepParent "Chromium.app" "$asset")
  verify_macos_app_zip "$asset" "Chromium.app"
  upload_chrome_asset "$asset"
}

package_linux_chrome_release() {
  require_linux build-release-chrome
  ensure_dirs

  local runtime stage asset
  runtime="$(find_local_chrome_runtime)" || fail "Linux Chromium runtime not found; set CHROME_RUNTIME_PATH or CHROMIUM_TINTIN_DIR"
  log "packaging Chromium Chrome runtime from $runtime"
  stage="$CACHE_DIR/stage/chromium-tintin-chrome-$(asset_platform)-$(asset_arch)"
  asset="$ARTIFACT_DIR/$(chrome_asset_name)"
  reset_dir "$stage"
  copy_linux_chrome_runtime "$runtime" "$stage"
  printf '%s\n' "$runtime" > "$stage/CHROME_RUNTIME_SOURCE.txt"
  "$stage/chrome" --version >&2 || fail "packaged Chromium Chrome failed --version"
  tar -C "$(dirname "$stage")" -czf "$asset" "$(basename "$stage")"
  upload_chrome_asset "$asset"
}

package_chrome_release() {
  case "$(asset_platform)" in
    macos) package_macos_chrome_release ;;
    linux) package_linux_chrome_release ;;
    *) fail "unsupported Chrome package platform: $(asset_platform)" ;;
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
  upload_cef_asset "$asset"
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

embed_macos_app_runtime() {
  local app="$1"
  local cef_runtime="$2"
  local executable
  require_command install_name_tool
  require_command rsync

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
  sign_macos_app_bundle "$app"
}

bundle_macos_app() {
  require_macos pack-macos
  require_command ditto
  build_macos
  local source_app app_name stage app cef_runtime asset
  source_app="$(mac_app_bundle)"
  [[ -n "$source_app" ]] || fail "Tauri macOS app bundle was not produced"
  app_name="$(basename "$source_app")"
  cef_runtime="$(ensure_cef_runtime_for_tauri)"
  stage="$CACHE_DIR/stage/macos-app"
  reset_dir "$stage"
  ditto "$source_app" "$stage/$app_name"
  app="$stage/$app_name"

  embed_macos_app_runtime "$app" "$cef_runtime"

  ensure_dirs
  asset="$ARTIFACT_DIR/$(desktop_asset_name)"
  (cd "$stage" && ditto -c -k --sequesterRsrc --keepParent "$app_name" "$asset")
  verify_macos_app_zip "$asset" "$app_name"
  upload_asset "$RELEASE_TAG" "$asset"

  if [[ "$UPLOAD_TUI_ARTIFACTS" == "1" ]]; then
    local tui_asset tui_stage
    tui_stage="$CACHE_DIR/stage/puffer-tui-$(asset_platform)-$(asset_arch)"
    tui_asset="$ARTIFACT_DIR/$(tui_asset_name)"
    reset_dir "$tui_stage"
    cp "$ROOT/target/release/puffer" "$tui_stage/puffer"
    tar -C "$(dirname "$tui_stage")" -czf "$tui_asset" "$(basename "$tui_stage")"
    upload_asset "$RELEASE_TAG" "$tui_asset"
  fi
}

pack_linux_local() {
  require_linux pack-linux-local
  build_linux
  ensure_dirs
  local stage asset appimage deb
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

  if [[ "$UPLOAD_TUI_ARTIFACTS" == "1" ]]; then
    local tui_asset tui_stage
    tui_stage="$CACHE_DIR/stage/puffer-tui-linux-$(asset_arch)"
    tui_asset="$ARTIFACT_DIR/$(tui_asset_name)"
    reset_dir "$tui_stage"
    cp "$ROOT/target/release/puffer" "$tui_stage/puffer"
    tar -C "$(dirname "$tui_stage")" -czf "$tui_asset" "$(basename "$tui_stage")"
    upload_asset "$RELEASE_TAG" "$tui_asset"
  fi
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
  git clone "git@github.com:$SOURCE_GITHUB_REPO.git" "$LINUX_REPO_DIR" || git clone "https://github.com/$SOURCE_GITHUB_REPO.git" "$LINUX_REPO_DIR"
fi
cd "$LINUX_REPO_DIR"
git fetch origin "$branch"
git checkout "$branch" || git checkout -b "$branch" "origin/$branch"
git pull --ff-only origin "$branch"
CHROMIUM_TINTIN_DIR="$LINUX_CHROMIUM_TINTIN_DIR" RELEASE_TAG="$RELEASE_TAG" CEF_RELEASE_TAG="$CEF_RELEASE_TAG" CHROME_RELEASE_TAG="$CHROME_RELEASE_TAG" SOURCE_GITHUB_REPO="$SOURCE_GITHUB_REPO" RELEASE_GITHUB_REPO="$RELEASE_GITHUB_REPO" CEF_GITHUB_REPO="$CEF_GITHUB_REPO" CHROME_GITHUB_REPO="$CHROME_GITHUB_REPO" UPLOAD_TUI_ARTIFACTS="$UPLOAD_TUI_ARTIFACTS" NO_UPLOAD=1 make build-release-cef
CHROMIUM_TINTIN_DIR="$LINUX_CHROMIUM_TINTIN_DIR" RELEASE_TAG="$RELEASE_TAG" CEF_RELEASE_TAG="$CEF_RELEASE_TAG" CHROME_RELEASE_TAG="$CHROME_RELEASE_TAG" SOURCE_GITHUB_REPO="$SOURCE_GITHUB_REPO" RELEASE_GITHUB_REPO="$RELEASE_GITHUB_REPO" CEF_GITHUB_REPO="$CEF_GITHUB_REPO" CHROME_GITHUB_REPO="$CHROME_GITHUB_REPO" UPLOAD_TUI_ARTIFACTS="$UPLOAD_TUI_ARTIFACTS" NO_UPLOAD=1 make build-release-chrome
CHROMIUM_TINTIN_DIR="$LINUX_CHROMIUM_TINTIN_DIR" RELEASE_TAG="$RELEASE_TAG" CEF_RELEASE_TAG="$CEF_RELEASE_TAG" CHROME_RELEASE_TAG="$CHROME_RELEASE_TAG" SOURCE_GITHUB_REPO="$SOURCE_GITHUB_REPO" RELEASE_GITHUB_REPO="$RELEASE_GITHUB_REPO" CEF_GITHUB_REPO="$CEF_GITHUB_REPO" CHROME_GITHUB_REPO="$CHROME_GITHUB_REPO" UPLOAD_TUI_ARTIFACTS="$UPLOAD_TUI_ARTIFACTS" NO_UPLOAD=1 make pack-linux-local
EOF

  remote_artifacts="$ARTIFACT_DIR/remote-linux"
  reset_dir "$remote_artifacts"
  rsync -av "$LINUX_HOST:$LINUX_REPO_DIR/release/" "$remote_artifacts/"
  local asset
  for asset in "$remote_artifacts"/puffer-cef-linux-*.tar.gz; do
    [[ -f "$asset" ]] || continue
    upload_cef_asset "$asset"
  done
  for asset in "$remote_artifacts"/chromium-tintin-chrome-linux-*.tar.gz; do
    [[ -f "$asset" ]] || continue
    upload_chrome_asset "$asset"
  done
  for asset in "$remote_artifacts"/puffer-desktop-linux-*.tar.gz; do
    [[ -f "$asset" ]] || continue
    upload_asset "$RELEASE_TAG" "$asset"
  done
  if [[ "$UPLOAD_TUI_ARTIFACTS" == "1" ]]; then
    for asset in "$remote_artifacts"/puffer-tui-linux-*.tar.gz; do
      [[ -f "$asset" ]] || continue
      upload_asset "$RELEASE_TAG" "$asset"
    done
  fi
}

case "${1:-help}" in
  help) usage ;;
  build-rust) build_rust ;;
  build-tauri) build_tauri ;;
  build-macos) build_macos ;;
  build-release-cef) build_release_cef ;;
  build-release-chrome) package_chrome_release ;;
  pack-macos) bundle_macos_app ;;
  build-linux) build_linux ;;
  pack-linux) pack_linux_remote ;;
  pack-linux-local) pack_linux_local ;;
  *) usage; fail "unknown release target: ${1:-}" ;;
esac
