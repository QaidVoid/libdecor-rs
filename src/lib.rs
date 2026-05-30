//! Client-side decorations for Wayland, in pure Rust.
//!
//! `libdecor` is a Rust reimplementation of the [libdecor] C library. It
//! takes care of the boilerplate around `xdg_wm_base` /
//! `xdg_surface` / `xdg_toplevel`, negotiates server-side decorations
//! through `xdg-decoration-unstable-v1` where supported, and falls back
//! to leaving the window undecorated otherwise. No GTK, no Cairo, no
//! Pango.
//!
//! # Quick start
//!
//! ```no_run
//! use libdecor::{Context, Event, State};
//!
//! let mut ctx = Context::connect().unwrap();
//! let frame_id = ctx.create_frame().unwrap();
//! {
//!     let mut frame = ctx.frame(frame_id).unwrap();
//!     frame.set_title("demo").unwrap();
//!     frame.set_app_id("io.example.demo").unwrap();
//!     frame.map().unwrap();
//! }
//!
//! loop {
//!     ctx.dispatch(None).unwrap();
//!     while let Some(event) = ctx.poll_event() {
//!         match event {
//!             Event::Configure { frame, configuration } => {
//!                 let (w, h) = configuration.content_size().unwrap_or((640, 480));
//!                 let state = State::new(w, h);
//!                 ctx.frame(frame)
//!                     .unwrap()
//!                     .commit(&state, Some(&configuration))
//!                     .unwrap();
//!             }
//!             Event::Close { .. } => return,
//!             _ => {}
//!         }
//!     }
//! }
//! ```
//!
//! [libdecor]: https://gitlab.freedesktop.org/libdecor/libdecor

#![deny(missing_docs)]

mod configuration;
mod context;
mod csd;
mod error;
mod event;
mod font;
mod frame;
mod id;
mod inner;
mod input;
mod shm;
mod state;
mod theme;

pub use configuration::Configuration;
pub use context::Context;
pub use error::{Error, Result};
pub use event::Event;
pub use frame::Frame;
pub use id::FrameId;
pub use shm::ShmBuffer;
pub use state::{Capabilities, ResizeEdge, State, WindowState, WmCapabilities};

/// Opaque dispatcher type used as the type parameter of
/// [`wayland_client::QueueHandle`] when creating additional Wayland
/// proxies (for example, `wl_shm_pool`) on libdecor's event queue.
pub use inner::Inner as Dispatcher;

pub use wayland_client;
pub use wayland_protocols;
