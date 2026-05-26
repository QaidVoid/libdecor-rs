//! C ABI shared library for libdecor-rs.
//!
//! Produces `libdecor-0.so.0`, a drop-in replacement for the upstream
//! libdecor shared library. Every exported symbol is `libdecor_*` and
//! mirrors the corresponding entry point in `<libdecor.h>`.
//!
//! All decoration logic lives in the parent [`libdecor`] crate; this
//! crate is the FFI shim that converts between C handles and Rust
//! objects.

#![allow(non_camel_case_types, non_upper_case_globals)]
// Safety contracts for every `libdecor_*` entry point follow the same
// recipe (handle pointers must be valid). Spell it out once here
// instead of repeating it on every function.
#![allow(clippy::missing_safety_doc)]

mod context;
mod frame;
mod state_config;
mod types;

pub use context::*;
pub use frame::*;
pub use state_config::*;
pub use types::*;
