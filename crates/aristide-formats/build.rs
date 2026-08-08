//! Links system libwavpack for `src/wavpack.rs`'s hand-written FFI.
//!
//! Prefers `pkg-config` (handles include/lib paths, and cross-distro
//! naming) and falls back to a plain `-lwavpack` for systems without a
//! `wavpack.pc` but with the library and headers on the default search
//! paths.

fn main() {
    if pkg_config::Config::new().probe("wavpack").is_ok() {
        return;
    }
    println!("cargo:rustc-link-lib=wavpack");
}
