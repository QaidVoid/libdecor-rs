//! FFI types matching `<libdecor.h>`: opaque handles, enums, vtables.

use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;

use libdecor_rs::{Capabilities, Context, FrameId, State, WindowState, WmCapabilities};

/// Opaque libdecor context. Layout-compatible with the `struct libdecor`
/// forward declaration in `<libdecor.h>`.
#[repr(C)]
pub struct libdecor {
    _opaque: [u8; 0],
}

/// Opaque libdecor frame.
#[repr(C)]
pub struct libdecor_frame {
    _opaque: [u8; 0],
}

/// Opaque libdecor state.
#[repr(C)]
pub struct libdecor_state {
    _opaque: [u8; 0],
}

/// Opaque libdecor configuration.
#[repr(C)]
pub struct libdecor_configuration {
    _opaque: [u8; 0],
}

/// Mirrors `enum libdecor_error`.
#[repr(C)]
#[allow(dead_code)]
pub enum libdecor_error {
    /// The compositor does not provide enough of the xdg-shell surface
    /// for libdecor to operate.
    COMPOSITOR_INCOMPATIBLE = 0,
    /// A frame is being committed with no usable state.
    INVALID_FRAME_CONFIGURATION = 1,
}

/// Mirrors `enum libdecor_resize_edge`.
#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq)]
#[allow(dead_code)]
pub enum libdecor_resize_edge {
    NONE = 0,
    TOP = 1,
    BOTTOM = 2,
    LEFT = 3,
    TOP_LEFT = 4,
    BOTTOM_LEFT = 5,
    RIGHT = 6,
    TOP_RIGHT = 7,
    BOTTOM_RIGHT = 8,
}

impl libdecor_resize_edge {
    pub(crate) fn to_rust(self) -> libdecor_rs::ResizeEdge {
        use libdecor_rs::ResizeEdge as R;
        match self {
            Self::NONE => R::None,
            Self::TOP => R::Top,
            Self::BOTTOM => R::Bottom,
            Self::LEFT => R::Left,
            Self::TOP_LEFT => R::TopLeft,
            Self::BOTTOM_LEFT => R::BottomLeft,
            Self::RIGHT => R::Right,
            Self::TOP_RIGHT => R::TopRight,
            Self::BOTTOM_RIGHT => R::BottomRight,
        }
    }
}

/// `enum libdecor_window_state` bitmask.
pub type libdecor_window_state = u32;

pub const LIBDECOR_WINDOW_STATE_NONE: libdecor_window_state = 0;

pub(crate) fn window_state_to_c(ws: WindowState) -> libdecor_window_state {
    ws.bits()
}

/// `enum libdecor_capabilities` bitmask.
pub type libdecor_capabilities = u32;

pub(crate) fn capabilities_to_c(c: Capabilities) -> libdecor_capabilities {
    c.bits()
}

pub(crate) fn capabilities_from_c(bits: libdecor_capabilities) -> Capabilities {
    Capabilities::from_bits_truncate(bits)
}

/// `enum libdecor_wm_capabilities` bitmask.
pub type libdecor_wm_capabilities = u32;

pub(crate) fn wm_capabilities_to_c(c: WmCapabilities) -> libdecor_wm_capabilities {
    c.bits()
}

/// Mirrors `struct libdecor_interface`. The first slot is the error
/// callback. The remaining slots are reserved by libdecor for ABI
/// expansion and are not invoked by this implementation.
#[repr(C)]
pub struct libdecor_interface {
    pub error:
        Option<unsafe extern "C" fn(*mut libdecor, error: libdecor_error, message: *const c_char)>,
    pub reserved0: Option<unsafe extern "C" fn()>,
    pub reserved1: Option<unsafe extern "C" fn()>,
    pub reserved2: Option<unsafe extern "C" fn()>,
    pub reserved3: Option<unsafe extern "C" fn()>,
    pub reserved4: Option<unsafe extern "C" fn()>,
    pub reserved5: Option<unsafe extern "C" fn()>,
    pub reserved6: Option<unsafe extern "C" fn()>,
    pub reserved7: Option<unsafe extern "C" fn()>,
    pub reserved8: Option<unsafe extern "C" fn()>,
    pub reserved9: Option<unsafe extern "C" fn()>,
}

/// Mirrors `struct libdecor_frame_interface`.
#[repr(C)]
pub struct libdecor_frame_interface {
    pub configure: Option<
        unsafe extern "C" fn(
            *mut libdecor_frame,
            *mut libdecor_configuration,
            user_data: *mut c_void,
        ),
    >,
    pub close: Option<unsafe extern "C" fn(*mut libdecor_frame, user_data: *mut c_void)>,
    pub commit: Option<unsafe extern "C" fn(*mut libdecor_frame, user_data: *mut c_void)>,
    pub dismiss_popup: Option<
        unsafe extern "C" fn(*mut libdecor_frame, seat_name: *const c_char, user_data: *mut c_void),
    >,
    pub bounds:
        Option<unsafe extern "C" fn(*mut libdecor_frame, width: c_int, height: c_int, *mut c_void)>,
    pub reserved0: Option<unsafe extern "C" fn()>,
    pub reserved1: Option<unsafe extern "C" fn()>,
    pub reserved2: Option<unsafe extern "C" fn()>,
    pub reserved3: Option<unsafe extern "C" fn()>,
    pub reserved4: Option<unsafe extern "C" fn()>,
    pub reserved5: Option<unsafe extern "C" fn()>,
    pub reserved6: Option<unsafe extern "C" fn()>,
    pub reserved7: Option<unsafe extern "C" fn()>,
    pub reserved8: Option<unsafe extern "C" fn()>,
}

/// Internal context state stored behind every `*mut libdecor`.
pub(crate) struct ContextBox {
    pub(crate) rust: Context,
    pub(crate) iface: NonNull<libdecor_interface>,
    pub(crate) user_data: *mut c_void,
    pub(crate) refs: u32,
    pub(crate) frames: std::collections::HashMap<FrameId, NonNull<FrameBox>>,
    pub(crate) handle_application_cursor: bool,
    /// Owned C-string buffers backing the pointer returned from
    /// [`libdecor_frame_get_title`]. The pointer must remain valid
    /// until the next title-changing call.
    pub(crate) title_cache: std::collections::HashMap<FrameId, std::ffi::CString>,
}

impl ContextBox {
    pub(crate) fn into_raw(self) -> *mut libdecor {
        Box::into_raw(Box::new(self)).cast()
    }

    /// # Safety
    ///
    /// `ptr` must be a valid pointer returned by [`Self::into_raw`].
    pub(crate) unsafe fn as_mut<'a>(ptr: *mut libdecor) -> Option<&'a mut Self> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *(ptr.cast::<ContextBox>()) })
        }
    }
}

/// Internal frame state stored behind every `*mut libdecor_frame`.
pub(crate) struct FrameBox {
    pub(crate) ctx: NonNull<ContextBox>,
    pub(crate) id: FrameId,
    pub(crate) iface: NonNull<libdecor_frame_interface>,
    pub(crate) user_data: *mut c_void,
    pub(crate) refs: u32,
}

impl FrameBox {
    pub(crate) fn into_raw(self) -> *mut libdecor_frame {
        Box::into_raw(Box::new(self)).cast()
    }

    /// # Safety
    ///
    /// `ptr` must be a valid pointer returned by [`Self::into_raw`].
    pub(crate) unsafe fn as_mut<'a>(ptr: *mut libdecor_frame) -> Option<&'a mut Self> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *(ptr.cast::<FrameBox>()) })
        }
    }
}

/// Internal state behind every `*mut libdecor_state`.
pub(crate) struct StateBox {
    pub(crate) rust: State,
}

impl StateBox {
    pub(crate) fn into_raw(self) -> *mut libdecor_state {
        Box::into_raw(Box::new(self)).cast()
    }

    /// # Safety
    ///
    /// `ptr` must be a valid pointer returned by [`Self::into_raw`].
    pub(crate) unsafe fn as_ref<'a>(ptr: *const libdecor_state) -> Option<&'a Self> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &*(ptr.cast::<StateBox>()) })
        }
    }
}

/// Internal state behind every `*mut libdecor_configuration`.
pub(crate) struct ConfigurationBox {
    pub(crate) rust: libdecor_rs::Configuration,
}

impl ConfigurationBox {
    pub(crate) fn into_raw(self) -> *mut libdecor_configuration {
        Box::into_raw(Box::new(self)).cast()
    }

    /// # Safety
    ///
    /// `ptr` must be a valid pointer returned by [`Self::into_raw`].
    pub(crate) unsafe fn as_ref<'a>(ptr: *const libdecor_configuration) -> Option<&'a Self> {
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &*(ptr.cast::<ConfigurationBox>()) })
        }
    }
}
