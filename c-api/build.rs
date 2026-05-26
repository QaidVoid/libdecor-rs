//! Linker glue for the cdylib that exposes libdecor's C ABI.
//!
//! The version script restricts exported symbols to the public C API
//! (`libdecor_*`) so the linker can dead-code-eliminate the rest, and
//! the soname is pinned to `libdecor-0.so.0` so the resulting binary
//! is a drop-in replacement for the upstream libdecor shared library.

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let script = format!("{manifest}/libdecor.ld");
    println!("cargo:rerun-if-changed=libdecor.ld");
    println!("cargo:rustc-cdylib-link-arg=-Wl,--version-script={script}");
    println!("cargo:rustc-cdylib-link-arg=-Wl,--gc-sections");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libdecor-0.so.0");
}
