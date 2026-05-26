//! Events surfaced to the application by [`Context::poll_event`].
//!
//! [`Context::poll_event`]: crate::Context::poll_event

use crate::configuration::Configuration;
use crate::id::FrameId;

/// An event produced by libdecor on behalf of a frame.
///
/// The application drives the library by repeatedly calling
/// [`Context::dispatch`](crate::Context::dispatch) and draining events
/// with [`Context::poll_event`](crate::Context::poll_event).
#[derive(Clone, Debug)]
pub enum Event {
    /// A new configuration was received. The application should derive a
    /// [`State`](crate::State) from `configuration`, redraw, and commit
    /// the frame via [`Frame::commit`](crate::Frame::commit).
    Configure {
        /// The frame this configuration applies to.
        frame: FrameId,
        /// Pending configuration. Pass this back to `commit`.
        configuration: Configuration,
    },

    /// The compositor requested that the window be closed.
    Close {
        /// The frame the close request applies to.
        frame: FrameId,
    },

    /// The decoration layer is asking the application to commit the main
    /// surface. This is fired when the decoration is implemented using
    /// synchronous subsurfaces.
    Commit {
        /// The frame to be committed.
        frame: FrameId,
    },

    /// A mapped popup with a grab on the given seat should be dismissed.
    DismissPopup {
        /// The frame the dismissal originates from.
        frame: FrameId,
        /// Name of the seat carrying the grab.
        seat_name: String,
    },

    /// The compositor delivered a recommended bounds rectangle for the
    /// window. A configure event will follow.
    Bounds {
        /// The frame the bounds apply to.
        frame: FrameId,
        /// Width in surface-local coordinates.
        width: i32,
        /// Height in surface-local coordinates.
        height: i32,
    },
}
