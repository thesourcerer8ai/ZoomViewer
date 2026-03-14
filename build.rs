fn main() {
    // For Windows builds, ensure proper linking
    #[cfg(target_os = "windows")]
    {
        // Link against msvcrt for C runtime functions
        println!("cargo:rustc-link-lib=msvcrt");
    }
}
