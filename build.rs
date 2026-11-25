fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // For Windows builds, ensure proper linking
    #[cfg(target_os = "windows")]
    {
        // Link against msvcrt for C runtime functions
        println!("cargo:rustc-link-lib=msvcrt");

        // WinFsp can only be delay-loaded; this emits the required
        // /DELAYLOAD:winfsp-x64.dll linker argument for the final binary.
        winfsp::build::winfsp_link_delayload();
    }
}
