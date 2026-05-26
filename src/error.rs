//! Error type for libdecor-rs.

use std::io;

use thiserror::Error;

/// Errors that can occur while talking to the Wayland compositor or
/// managing decorations.
#[derive(Debug, Error)]
pub enum Error {
    /// The Wayland connection could not be established. Usually this means
    /// `$WAYLAND_DISPLAY` is unset or the compositor socket cannot be opened.
    #[error("failed to connect to wayland display: {0}")]
    Connect(#[from] wayland_client::ConnectError),

    /// Communication with the compositor failed while dispatching events.
    #[error("wayland dispatch failed: {0}")]
    Dispatch(#[from] wayland_client::DispatchError),

    /// The Wayland backend returned an error (for example, a protocol
    /// violation or a closed socket).
    #[error("wayland backend error: {0}")]
    Backend(#[from] wayland_client::backend::WaylandError),

    /// The compositor advertised globals that are incompatible with
    /// libdecor-rs (for example, no `xdg_wm_base`).
    #[error("compositor missing required global: {0}")]
    MissingGlobal(&'static str),

    /// A bind to a global failed.
    #[error("failed to bind wayland global: {0}")]
    Bind(#[from] wayland_client::globals::BindError),

    /// Global registry could not be initialised.
    #[error("failed to register wayland globals: {0}")]
    Globals(#[from] wayland_client::globals::GlobalError),

    /// An I/O error occurred while preparing a buffer or talking to the
    /// compositor.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),

    /// A frame id passed by the caller does not refer to a live frame.
    #[error("unknown frame id")]
    UnknownFrame,
}

impl From<rustix::io::Errno> for Error {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Io(value.into())
    }
}

/// Crate-wide `Result` alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
