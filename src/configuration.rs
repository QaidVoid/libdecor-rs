//! Per-configure-event data received from the compositor.

use crate::state::WindowState;

/// A pending configuration delivered alongside an
/// [`Event::Configure`](crate::Event::Configure).
///
/// The client should derive a [`State`](crate::State) from this and pass
/// both to [`Frame::commit`](crate::Frame::commit).
#[derive(Clone, Debug)]
pub struct Configuration {
    pub(crate) serial: u32,
    pub(crate) size: Option<(i32, i32)>,
    pub(crate) window_state: Option<WindowState>,
    pub(crate) bounds: Option<(i32, i32)>,
}

impl Configuration {
    /// The xdg_surface configure serial that this configuration
    /// corresponds to.
    pub const fn serial(&self) -> u32 {
        self.serial
    }

    /// The expected content size for this configuration, if the
    /// compositor specified one. If `None`, the client should keep its
    /// current size or pick its own.
    pub const fn content_size(&self) -> Option<(i32, i32)> {
        self.size
    }

    /// The window state for this configuration, if changed.
    pub const fn window_state(&self) -> Option<WindowState> {
        self.window_state
    }

    /// The compositor-suggested bounds for the window, if delivered via
    /// `xdg_toplevel.configure_bounds`.
    pub const fn bounds(&self) -> Option<(i32, i32)> {
        self.bounds
    }
}
