//! `libdecor_state` and `libdecor_configuration` getters.

use core::ffi::{c_int, c_void};

use libdecor_rs::State;

use crate::types::{
    ConfigurationBox, StateBox, libdecor_configuration, libdecor_frame, libdecor_state,
    libdecor_window_state, window_state_to_c,
};

/// Allocate a new content state describing the requested
/// `width` x `height` content size.
#[unsafe(no_mangle)]
pub extern "C" fn libdecor_state_new(width: c_int, height: c_int) -> *mut libdecor_state {
    StateBox {
        rust: State::new(width, height),
    }
    .into_raw()
}

/// Free a previously allocated state.
///
/// # Safety
///
/// `state` must be `NULL` or a pointer returned by [`libdecor_state_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_state_free(state: *mut libdecor_state) {
    if state.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(state.cast::<StateBox>()) };
}

/// Write the configuration's recommended content size into the
/// `width` and `height` out-params. Returns `false` when the
/// configuration carries no explicit size.
///
/// # Safety
///
/// `configuration` must be the pointer delivered to the latest
/// configure callback. `width` and `height` must point to writable
/// `int` storage. `frame` is accepted for API compatibility and may be
/// `NULL`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_configuration_get_content_size(
    configuration: *mut libdecor_configuration,
    _frame: *mut libdecor_frame,
    width: *mut c_int,
    height: *mut c_int,
) -> bool {
    let Some(cfg) = (unsafe { ConfigurationBox::as_ref(configuration) }) else {
        return false;
    };
    let Some((w, h)) = cfg.rust.content_size() else {
        return false;
    };
    if !width.is_null() {
        unsafe { *width = w };
    }
    if !height.is_null() {
        unsafe { *height = h };
    }
    true
}

/// Write the configuration's window state bitmask into `window_state`.
/// Returns `false` when no state change is described.
///
/// # Safety
///
/// `configuration` must be the pointer delivered to the latest
/// configure callback. `window_state` must point to writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_configuration_get_window_state(
    configuration: *mut libdecor_configuration,
    window_state: *mut libdecor_window_state,
) -> bool {
    let Some(cfg) = (unsafe { ConfigurationBox::as_ref(configuration) }) else {
        return false;
    };
    let Some(ws) = cfg.rust.window_state() else {
        return false;
    };
    if !window_state.is_null() {
        unsafe { *window_state = window_state_to_c(ws) };
    }
    true
}

#[allow(dead_code)]
pub(crate) fn as_state_ref(state: *const libdecor_state) -> Option<&'static State> {
    unsafe { StateBox::as_ref(state) }.map(|b| {
        // Lifetime is bounded by the caller's scope; we erase the
        // lifetime here because this is a private helper invoked
        // synchronously by FFI entry points that already hold the
        // pointer for the duration of the call.
        let r: &'static State = unsafe { &*((&b.rust as *const State).cast::<State>()) };
        r
    })
}

/// Read the State stored behind a `libdecor_state` pointer.
pub(crate) fn state_of(state: *const libdecor_state) -> Option<State> {
    unsafe { StateBox::as_ref(state) }.map(|b| b.rust)
}

/// Hint to the linker / static analyzers that the FFI module is in use.
#[allow(dead_code)]
fn _suppress_unused_warning(_: *mut c_void) {}
