//! Context lifecycle (`libdecor_new` / `libdecor_unref` / `libdecor_dispatch`).

use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;
use core::time::Duration;

use crate::dispatch::ContextHandle;
use crate::frame::free_frame_box;
use crate::types::{ContextBox, FrameBox, libdecor, libdecor_interface};

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

    // Spawn the worker that services libdecor's private queue.
    let handle = match ContextHandle::new(ctx) {
        Ok(h) => h,
        Err(_) => return core::ptr::null_mut(),
    };

    let boxed = ContextBox {
        rust: handle,
        iface: iface_nn,
        user_data,
        refs: 1,
        pump: core::ptr::null_mut(),
        frames: std::collections::HashMap::new(),
        handle_application_cursor: false,
        title_cache: std::collections::HashMap::new(),
    };
    let raw = boxed.into_raw();
    // Arm the default-queue delivery pump now that the box has a stable
    // address (the pump callback's user_data points back at it).
    let ctx_ptr = raw.cast::<ContextBox>();
    unsafe { (*ctx_ptr).pump = crate::pump::create(ctx_ptr, display) };
    raw
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
        // Stop the worker first: the application may disconnect the display
        // immediately after this returns (the mesa close callback does),
        // and the worker must not be touching it.
        boxed.rust.shutdown();
        // Detach the default-queue pump. When we are unwinding through the
        // pump's own callback, this keeps the pump alive for it to finish.
        unsafe { crate::pump::detach(boxed) };
        let frames: Vec<NonNull<FrameBox>> = boxed.frames.values().copied().collect();
        for frame in frames {
            unsafe { free_frame_box(frame) };
        }
        // Drop the box (and its Wayland-owning Context) synchronously,
        // while the display is still valid.
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

/// Return the Wayland socket file descriptor, mirroring upstream
/// libdecor.
///
/// Applications that poll this and call [`libdecor_dispatch`] when it is
/// readable work as they would with upstream libdecor. Applications that
/// never do (some, like mesa's `eglut`, don't) are still served: libdecor
/// delivers events from the application's own default-queue dispatch via
/// [`crate::pump`].
///
/// # Safety
///
/// `ctx` must be a valid context handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_get_fd(ctx: *mut libdecor) -> c_int {
    match unsafe { ContextBox::as_mut(ctx) } {
        Some(b) => b.rust.socket_fd(),
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

    // Read the socket, dispatch the private queue, and deliver. This is
    // what services the application's initial `while (!configured)` loop,
    // which spins on `libdecor_dispatch` before it ever pumps the default
    // queue (so the pump cannot fire yet). In the main loop the pump takes
    // over; whichever drains first wins, so events are delivered exactly
    // once. The events are collected before invoking callbacks so each may
    // freely re-enter the C ABI.
    let events = boxed.rust.dispatch_read(timeout);
    unsafe { crate::pump::deliver_events(boxed, events) }
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
