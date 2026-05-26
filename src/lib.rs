//! Client-side decorations for Wayland, in pure Rust.
//!
//! `libdecor` is a Rust reimplementation of the [libdecor] C library.
//! Work in progress: this commit lays down the public value types
//! (errors, window state, capabilities, configuration, events). The
//! `Context` / `Frame` implementation arrives in a follow-up.
//!
//! [libdecor]: https://gitlab.freedesktop.org/libdecor/libdecor

#![deny(missing_docs)]

mod configuration;
mod error;
mod event;
mod id;
mod state;

pub use configuration::Configuration;
pub use error::{Error, Result};
pub use event::Event;
pub use id::FrameId;
pub use state::{Capabilities, ResizeEdge, State, WindowState, WmCapabilities};

pub use wayland_client;
pub use wayland_protocols;
