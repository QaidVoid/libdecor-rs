//! Linker glue for the cdylib that exposes libdecor's C ABI.
//!
//! rustc already restricts exported symbols to the public C API
//! (`libdecor_*`), and the soname is pinned to `libdecor-0.so.0` so the
//! resulting binary is a drop-in replacement for the upstream libdecor
//! shared library.

fn main() {
    println!("cargo:rustc-cdylib-link-arg=-Wl,--gc-sections");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libdecor-0.so.0");
}
