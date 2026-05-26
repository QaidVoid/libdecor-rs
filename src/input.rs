//! Seat and pointer input plumbing.
//!
//! libdecor manages its own `wl_pointer` per seat so it can react to
//! clicks and drags on the decoration subsurfaces it owns. Application
//! input on the content surface is left to the application's own
//! pointer objects.
//!
//! The dispatcher tracks which `wl_surface` the pointer is currently
//! over and what part of a frame that surface represents (content,
//! titlebar, or a resize border). CSD layers consume this state to
//! drive hit testing.

use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_touch::WlTouch};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;

use crate::id::FrameId;

/// Which part of a frame a pointer (or other input) is interacting
/// with. Tracked per `wl_surface` so we can route pointer events back
/// to the correct frame.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum DecorationPart {
    /// The content surface owned by the application.
    Content,
    /// The titlebar subsurface above the content.
    Titlebar,
    /// One of the four resize border subsurfaces.
    Border(BorderEdge),
}

/// Edge a resize border belongs to.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum BorderEdge {
    /// Top border above the titlebar.
    Top,
    /// Bottom border below the content.
    Bottom,
    /// Left border alongside the content.
    Left,
    /// Right border alongside the content.
    Right,
}

/// Mapping from a `wl_surface` (by id) to the frame and decoration
/// part it represents.
#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct SurfaceTarget {
    pub(crate) frame: FrameId,
    pub(crate) part: DecorationPart,
}

/// Per-seat input device handles.
pub(crate) struct SeatState {
    pub(crate) pointer: Option<WlPointer>,
    /// `wp_cursor_shape_device_v1` for the seat's pointer, if the
    /// compositor advertises `wp_cursor_shape_manager_v1`.
    pub(crate) cursor_shape: Option<WpCursorShapeDeviceV1>,
    #[allow(dead_code)]
    pub(crate) keyboard: Option<WlKeyboard>,
    #[allow(dead_code)]
    pub(crate) touch: Option<WlTouch>,
}

impl SeatState {
    pub(crate) fn new() -> Self {
        Self {
            pointer: None,
            cursor_shape: None,
            keyboard: None,
            touch: None,
        }
    }
}

/// Where a pointer is currently focused, with the latest surface-local
/// position. `None` means the pointer is not over any libdecor-owned
/// surface.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct PointerFocus {
    pub(crate) target: SurfaceTarget,
    pub(crate) surface_id: ObjectId,
    pub(crate) serial: u32,
    pub(crate) x: f64,
    pub(crate) y: f64,
    /// Whether the primary mouse button is currently pressed.
    pub(crate) button_down: bool,
}
