use std::env;
use std::fs;
use std::path::PathBuf;

// Vendored copy of `winfsp-sys` (crates.io v0.12.1+winfsp-2.1).
//
// The upstream build script runs `bindgen` over the vendored WinFsp SDK
// headers, which requires libclang (LLVM) to be installed on the build
// machine. This crate already ships the *generated* bindings in
// `src/bindings.rs`, so we copy them into OUT_DIR instead and skip bindgen
// entirely. That keeps the build free of an LLVM dependency while producing
// exactly the same FFI surface.
//
// The WinFsp SDK (headers, import libraries and the delay-load DLLs) is
// vendored under `winfsp/`, so no WinFsp installation is needed to *build*;
// the runtime driver is still required to actually mount a volume.

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        panic!("WinFSP is only supported on Windows.");
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    let winfsp_lib = match (target_arch.as_str(), target_env.as_str()) {
        ("x86_64", "msvc") => "winfsp-x64",
        ("x86", "msvc") => "winfsp-x86",
        ("aarch64", "msvc") => "winfsp-a64",
        _ => panic!("unsupported triple {}-{}", target_arch, target_env),
    };

    // Link the vendored WinFsp import library. WinFsp only supports
    // delay-loading, hence the `delayimp` helper library and the
    // /DELAYLOAD: linker argument.
    println!("cargo:rustc-link-search={}", manifest_dir.join("winfsp/lib").to_string_lossy());
    println!("cargo:rustc-link-lib=dylib=delayimp");
    println!("cargo:rustc-link-lib=dylib={}", winfsp_lib);
    println!("cargo:rustc-link-arg=/DELAYLOAD:{}.dll", winfsp_lib);

    // Use the pre-generated bindings instead of running bindgen.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bindings.rs");
    let bindings = manifest_dir.join("src/bindings.rs");
    let dest = out_dir.join("bindings.rs");
    fs::copy(&bindings, &dest)
        .unwrap_or_else(|e| panic!("failed to copy pre-generated bindings: {e}"));
}
