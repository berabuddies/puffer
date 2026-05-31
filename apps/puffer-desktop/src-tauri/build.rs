use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(puffer_desktop_cef_native)");
    #[cfg(target_os = "macos")]
    build_macos_cef_bridge();
    tauri_build::build()
}

#[cfg(target_os = "macos")]
fn build_macos_cef_bridge() {
    emit_cef_rerun_hints();
    let Some(paths) = CefBuildPaths::discover() else {
        return;
    };
    println!("cargo:rustc-cfg=puffer_desktop_cef_native");
    println!(
        "cargo:rustc-env=PUFFER_DESKTOP_CEF_ROOT={}",
        paths.runtime_root.display()
    );
    println!(
        "cargo:rustc-env=PUFFER_DESKTOP_CEF_HELPER={}",
        paths.helper_executable.display()
    );
    println!("cargo:rerun-if-changed=src/cef_host_mac.mm");

    ensure_dev_framework_link(&paths.runtime_root);
    ensure_helper_library_links(&paths.runtime_root, &paths.helper_executable);
    compile_cef_wrapper(&paths.cef_root);

    cc::Build::new()
        .cpp(true)
        .file("src/cef_host_mac.mm")
        .include(&paths.cef_root)
        .flag("-std=c++20")
        .flag("-fobjc-arc")
        .flag("-Wno-missing-field-initializers")
        .flag("-Wno-unused-parameter")
        .compile("puffer_desktop_cef_host");

    println!(
        "cargo:rustc-link-search=framework={}",
        paths.runtime_root.display()
    );
    println!("cargo:rustc-link-lib=framework=Chromium Embedded Framework");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=c++");
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        paths.runtime_root.display()
    );
}

#[cfg(target_os = "macos")]
fn emit_cef_rerun_hints() {
    println!("cargo:rerun-if-env-changed=PUFFER_CEF_PATH");
    println!("cargo:rerun-if-env-changed=PUFFER_CEF_ROOT");
    println!("cargo:rerun-if-env-changed=CEF_PATH");
    println!("cargo:rerun-if-changed=target/puffer-cef-runtime");
}

#[cfg(target_os = "macos")]
struct CefBuildPaths {
    cef_root: PathBuf,
    runtime_root: PathBuf,
    helper_executable: PathBuf,
}

#[cfg(target_os = "macos")]
impl CefBuildPaths {
    fn discover() -> Option<Self> {
        for root in candidate_roots() {
            if let Some(paths) = Self::from_runtime_root(&root) {
                return Some(paths);
            }
        }
        None
    }

    fn from_runtime_root(root: &Path) -> Option<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let framework_binary = root
            .join("Chromium Embedded Framework.framework")
            .join("Chromium Embedded Framework");
        if !framework_binary.is_file() {
            return None;
        }
        let src_root = chromium_src_root(&root)?;
        let cef_root = packaged_cef_root(&src_root).unwrap_or_else(|| src_root.join("cef"));
        let helper_executable = root.join("cefsimple Helper.app/Contents/MacOS/cefsimple Helper");
        if cef_root.join("include/cef_app.h").is_file()
            && cef_root.join("include/cef_config.h").is_file()
            && cef_root
                .join("libcef_dll/wrapper/libcef_dll_wrapper.cc")
                .is_file()
            && helper_executable.is_file()
        {
            return Some(Self {
                cef_root,
                runtime_root: root.to_path_buf(),
                helper_executable,
            });
        }
        None
    }
}

#[cfg(target_os = "macos")]
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in ["PUFFER_CEF_PATH", "PUFFER_CEF_ROOT", "CEF_PATH"] {
        if let Some(path) = std::env::var_os(key) {
            add_root_candidates(&mut roots, PathBuf::from(path));
        }
    }
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let root = PathBuf::from(manifest_dir).join("target/puffer-cef-runtime");
        add_root_candidates(&mut roots, root);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let root = PathBuf::from(home).join("chromium_tintin/src/out/Release_GN_arm64");
        add_root_candidates(&mut roots, root);
    }
    roots
}

#[cfg(target_os = "macos")]
fn add_root_candidates(roots: &mut Vec<PathBuf>, root: PathBuf) {
    roots.push(root.clone());
    roots.push(root.join("Release_GN_arm64"));
    roots.push(root.join("Release"));
}

#[cfg(target_os = "macos")]
fn chromium_src_root(root: &Path) -> Option<PathBuf> {
    let mut current = root;
    while let Some(parent) = current.parent() {
        if parent.join("cef/include/cef_app.h").is_file() {
            return Some(parent.to_path_buf());
        }
        current = parent;
    }
    None
}

#[cfg(target_os = "macos")]
fn packaged_cef_root(src_root: &Path) -> Option<PathBuf> {
    let output_dir = src_root.parent()?.join("output");
    let entries = std::fs::read_dir(output_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("cef_binary_")
        {
            continue;
        }
        if path.join("include/cef_config.h").is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn compile_cef_wrapper(cef_root: &Path) {
    let mut sources = Vec::new();
    collect_cef_wrapper_sources(&cef_root.join("libcef_dll"), &mut sources);
    sources.sort();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(cef_root)
        .define("__STDC_CONSTANT_MACROS", None)
        .define("__STDC_FORMAT_MACROS", None)
        .define("WRAPPING_CEF_SHARED", None)
        .flag("-std=c++20")
        .flag("-fno-exceptions")
        .flag("-fno-rtti")
        .flag("-fno-threadsafe-statics")
        .flag("-fobjc-call-cxx-cdtors")
        .flag("-fvisibility=hidden")
        .flag("-fvisibility-inlines-hidden")
        .flag("-Wno-missing-field-initializers")
        .flag("-Wno-unused-parameter")
        .flag("-Wno-narrowing")
        .flag("-Wno-sign-compare")
        .flag("-Wno-undefined-var-template")
        .flag("-mmacosx-version-min=12.0");
    for source in sources {
        build.file(source);
    }
    build.compile("puffer_desktop_cef_wrapper");
}

#[cfg(target_os = "macos")]
fn collect_cef_wrapper_sources(dir: &Path, sources: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_cef_wrapper_sources(&path, sources);
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if extension == "cc" || extension == "mm" {
            println!("cargo:rerun-if-changed={}", path.display());
            sources.push(path);
        }
    }
}

#[cfg(target_os = "macos")]
fn ensure_dev_framework_link(runtime_root: &Path) {
    let Some(target_dirs) = cargo_target_dirs() else {
        return;
    };
    for target_dir in target_dirs {
        ensure_dev_framework_link_at(runtime_root, &target_dir);
    }
}

#[cfg(target_os = "macos")]
fn ensure_dev_framework_link_at(runtime_root: &Path, target_dir: &Path) {
    let framework = runtime_root.join("Chromium Embedded Framework.framework");
    let frameworks_dir = target_dir.join("Frameworks");
    let link = frameworks_dir.join("Chromium Embedded Framework.framework");
    if std::fs::symlink_metadata(&link).is_ok() {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(&frameworks_dir) {
        println!(
            "cargo:warning=failed to create CEF Frameworks directory {}: {error}",
            frameworks_dir.display()
        );
        return;
    }
    if let Err(error) = std::os::unix::fs::symlink(&framework, &link) {
        println!(
            "cargo:warning=failed to link CEF framework {} -> {}: {error}",
            link.display(),
            framework.display()
        );
    }
}

#[cfg(target_os = "macos")]
fn ensure_helper_library_links(runtime_root: &Path, helper_executable: &Path) {
    let Some(helper_dir) = helper_executable.parent() else {
        return;
    };
    let libraries_dir = runtime_root
        .join("Chromium Embedded Framework.framework")
        .join("Libraries");
    for name in [
        "libEGL.dylib",
        "libGLESv2.dylib",
        "libvk_swiftshader.dylib",
        "vk_swiftshader_icd.json",
    ] {
        ensure_symlink(&libraries_dir.join(name), &helper_dir.join(name));
    }
}

#[cfg(target_os = "macos")]
fn ensure_symlink(source: &Path, link: &Path) {
    if !source.is_file() || std::fs::symlink_metadata(link).is_ok() {
        return;
    }
    if let Err(error) = std::os::unix::fs::symlink(source, link) {
        println!(
            "cargo:warning=failed to link CEF runtime file {} -> {}: {error}",
            link.display(),
            source.display()
        );
    }
}

#[cfg(target_os = "macos")]
fn cargo_target_dirs() -> Option<Vec<PathBuf>> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    let profile_dir = out_dir.ancestors().nth(3)?.to_path_buf();
    let workspace_target_dir = profile_dir.parent()?.to_path_buf();
    Some(vec![workspace_target_dir, profile_dir])
}
