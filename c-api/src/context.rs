//! Context lifecycle (`libdecor_new` / `libdecor_unref` / `libdecor_dispatch`).

use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;
use core::time::Duration;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;

use libdecor_rs::Event;

use crate::frame::{free_frame_box, invoke_close, invoke_configure};
use crate::types::{ConfigurationBox, ContextBox, FrameBox, libdecor, libdecor_interface};

/// Create a new libdecor context for the given `*mut wl_display`.
///
/// Returns NULL on failure (e.g. when the compositor cannot be talked
/// to or the required Wayland globals are missing).
///
/// # Safety
///
/// `display` must be a valid `*mut wl_display`. `iface` must point to a
/// valid `libdecor_interface` that lives at least as long as the
/// returned context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_new(
    display: *mut c_void,
    iface: *mut libdecor_interface,
) -> *mut libdecor {
    unsafe { libdecor_new_with_user_data(display, iface, core::ptr::null_mut()) }
}

/// Variant of [`libdecor_new`] that also attaches an opaque user
/// data pointer accessible via [`libdecor_get_user_data`].
///
/// # Safety
///
/// See [`libdecor_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_new_with_user_data(
    display: *mut c_void,
    iface: *mut libdecor_interface,
    user_data: *mut c_void,
) -> *mut libdecor {
    let Some(iface_nn) = NonNull::new(iface) else {
        return core::ptr::null_mut();
    };
    if display.is_null() {
        return core::ptr::null_mut();
    }

    let ctx = match unsafe { libdecor_rs::Context::from_display(display) } {
        Ok(c) => c,
        Err(_) => return core::ptr::null_mut(),
    };

    let boxed = ContextBox {
        rust: ctx,
        iface: iface_nn,
        user_data,
        refs: 1,
        frames: std::collections::HashMap::new(),
        handle_application_cursor: false,
        title_cache: std::collections::HashMap::new(),
    };
    boxed.into_raw()
}

/// Decrement the context's reference count. The context (and any
/// remaining frames) is freed when the count reaches zero.
///
/// # Safety
///
/// `ctx` must have been returned by [`libdecor_new`] (or
/// [`libdecor_new_with_user_data`]) and must not have been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_unref(ctx: *mut libdecor) {
    let Some(boxed) = (unsafe { ContextBox::as_mut(ctx) }) else {
        return;
    };
    boxed.refs = boxed.refs.saturating_sub(1);
    if boxed.refs == 0 {
        let frames: Vec<NonNull<FrameBox>> = boxed.frames.values().copied().collect();
        for frame in frames {
            unsafe { free_frame_box(frame) };
        }
        drop(unsafe { Box::from_raw(ctx.cast::<ContextBox>()) });
    }
}

/// Get the user data pointer attached to this context.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_get_user_data(ctx: *mut libdecor) -> *mut c_void {
    match unsafe { ContextBox::as_mut(ctx) } {
        Some(b) => b.user_data,
        None => core::ptr::null_mut(),
    }
}

/// Replace the user data pointer attached to this context.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_set_user_data(ctx: *mut libdecor, user_data: *mut c_void) {
    if let Some(b) = unsafe { ContextBox::as_mut(ctx) } {
        b.user_data = user_data;
    }
}

/// Return the Wayland connection file descriptor.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_get_fd(ctx: *mut libdecor) -> c_int {
    match unsafe { ContextBox::as_mut(ctx) } {
        Some(b) => b.rust.as_fd().as_raw_fd(),
        None => -1,
    }
}

/// Dispatch any pending Wayland events, blocking for up to `timeout`
/// milliseconds (`-1` blocks indefinitely; `0` polls without blocking).
///
/// Returns the number of frame events dispatched, or a negative value
/// on error.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_dispatch(ctx: *mut libdecor, timeout: c_int) -> c_int {
    let Some(boxed) = (unsafe { ContextBox::as_mut(ctx) }) else {
        return -1;
    };

    let timeout = if timeout < 0 {
        None
    } else {
        Some(Duration::from_millis(timeout as u64))
    };

    if let Err(_e) = boxed.rust.dispatch(timeout) {
        return -1;
    }

    let mut dispatched: c_int = 0;
    while let Some(event) = boxed.rust.poll_event() {
        match event {
            Event::Configure {
                frame,
                configuration,
            } => {
                let Some(frame_ptr) = boxed.frames.get(&frame).copied() else {
                    continue;
                };
                let cfg = ConfigurationBox {
                    rust: configuration,
                }
                .into_raw();
                unsafe { invoke_configure(frame_ptr, cfg) };
                let _ = unsafe { Box::from_raw(cfg.cast::<ConfigurationBox>()) };
                dispatched += 1;
            }
            Event::Close { frame } => {
                if let Some(frame_ptr) = boxed.frames.get(&frame).copied() {
                    unsafe { invoke_close(frame_ptr) };
                    dispatched += 1;
                }
            }
            Event::Commit { frame: _ } | Event::DismissPopup { .. } | Event::Bounds { .. } => {
                // Not currently produced by our event source.
            }
        }
    }
    dispatched
}

/// Configure whether libdecor sets the default cursor when the pointer
/// is over an application surface. Currently a no-op.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_set_handle_application_cursor(
    ctx: *mut libdecor,
    handle_cursor: bool,
) {
    if let Some(b) = unsafe { ContextBox::as_mut(ctx) } {
        b.handle_application_cursor = handle_cursor;
    }
}

/// Invoke the context's error callback. Used internally for surfacing
/// non-fatal compositor issues.
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[allow(dead_code)]
pub(crate) unsafe fn report_error(
    ctx: &mut ContextBox,
    error: crate::types::libdecor_error,
    message: *const c_char,
) {
    if let Some(cb) = unsafe { ctx.iface.as_ref().error } {
        unsafe { cb((ctx as *mut ContextBox).cast::<libdecor>(), error, message) };
    }
}
