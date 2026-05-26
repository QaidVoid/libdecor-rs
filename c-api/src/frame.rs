//! Frame creation and per-window control (`libdecor_frame_*`).

use core::ffi::{c_char, c_int, c_void};
use core::ptr::NonNull;

use libdecor_rs::wayland_client::Proxy;
use libdecor_rs::wayland_client::backend::ObjectId;
use libdecor_rs::wayland_client::protocol::{
    wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
};
use libdecor_rs::wayland_protocols::xdg::shell::client::{
    xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel,
};

use crate::state_config::state_of;
use crate::types::{
    ConfigurationBox, ContextBox, FrameBox, capabilities_from_c, capabilities_to_c, libdecor,
    libdecor_capabilities, libdecor_configuration, libdecor_frame, libdecor_frame_interface,
    libdecor_resize_edge, libdecor_state, libdecor_wm_capabilities, wm_capabilities_to_c,
};

/// Decorate a `wl_surface` previously created by the client. Returns
/// NULL on failure (for example, when the surface pointer cannot be
/// reflected back through libwayland-client or libdecor's own internal
/// allocation fails).
///
/// # Safety
///
/// `context` must be a valid context handle. `surface` must be a live
/// `*mut wl_surface`. `iface` must point to a valid interface struct
/// that lives at least as long as the returned frame. `user_data` is
/// opaque and passed back to each callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_decorate(
    context: *mut libdecor,
    surface: *mut c_void,
    iface: *mut libdecor_frame_interface,
    user_data: *mut c_void,
) -> *mut libdecor_frame {
    let Some(ctx) = (unsafe { ContextBox::as_mut(context) }) else {
        return core::ptr::null_mut();
    };
    let Some(iface_nn) = NonNull::new(iface) else {
        return core::ptr::null_mut();
    };
    if surface.is_null() {
        return core::ptr::null_mut();
    }

    let id = match unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) } {
        Ok(i) => i,
        Err(_) => return core::ptr::null_mut(),
    };
    let wl_surface = match WlSurface::from_id(ctx.rust.connection(), id) {
        Ok(s) => s,
        Err(_) => return core::ptr::null_mut(),
    };

    let frame_id = match ctx.rust.decorate(wl_surface) {
        Ok(id) => id,
        Err(_) => return core::ptr::null_mut(),
    };

    let frame_box = FrameBox {
        ctx: NonNull::new(context.cast::<ContextBox>()).expect("non-null context"),
        id: frame_id,
        iface: iface_nn,
        user_data,
        refs: 1,
    };
    let raw = frame_box.into_raw();
    ctx.frames
        .insert(frame_id, NonNull::new(raw.cast::<FrameBox>()).unwrap());
    raw
}

/// Increment the frame's reference count.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_ref(frame: *mut libdecor_frame) {
    if let Some(b) = unsafe { FrameBox::as_mut(frame) } {
        b.refs = b.refs.saturating_add(1);
    }
}

/// Decrement the frame's reference count and free it when it reaches
/// zero.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_unref(frame: *mut libdecor_frame) {
    let Some(b) = (unsafe { FrameBox::as_mut(frame) }) else {
        return;
    };
    b.refs = b.refs.saturating_sub(1);
    if b.refs == 0 {
        let frame_id = b.id;
        let ctx = b.ctx;
        unsafe {
            (*ctx.as_ptr()).frames.remove(&frame_id);
            let _ = (*ctx.as_ptr()).rust.destroy_frame(frame_id);
            free_frame_box(NonNull::new(frame.cast::<FrameBox>()).unwrap());
        }
    }
}

/// # Safety
///
/// `frame` must be a non-null pointer to a [`FrameBox`] that is no
/// longer referenced from any context or callback.
pub(crate) unsafe fn free_frame_box(frame: NonNull<FrameBox>) {
    let _ = unsafe { Box::from_raw(frame.as_ptr()) };
}

/// Get the user data attached to this frame.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_user_data(frame: *mut libdecor_frame) -> *mut c_void {
    match unsafe { FrameBox::as_mut(frame) } {
        Some(b) => b.user_data,
        None => core::ptr::null_mut(),
    }
}

/// Replace the user data attached to this frame.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_user_data(
    frame: *mut libdecor_frame,
    user_data: *mut c_void,
) {
    if let Some(b) = unsafe { FrameBox::as_mut(frame) } {
        b.user_data = user_data;
    }
}

/// Set the decoration visibility flag.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_visibility(frame: *mut libdecor_frame, visible: bool) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_visibility(visible);
        }
    });
}

/// Return whether the frame's decorations are currently visible.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_is_visible(frame: *mut libdecor_frame) -> bool {
    with_frame_ret(frame, false, |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.is_visible().ok())
            .unwrap_or(false)
    })
}

/// Stack this frame above its parent.
///
/// # Safety
///
/// `frame` and `parent` (when not NULL) must be valid frame handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_parent(
    frame: *mut libdecor_frame,
    parent: *mut libdecor_frame,
) {
    let parent_id = unsafe { FrameBox::as_mut(parent) }.map(|p| p.id);
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_parent(parent_id);
        }
    });
}

/// Set the window title.
///
/// # Safety
///
/// `title` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_title(
    frame: *mut libdecor_frame,
    title: *const c_char,
) {
    if title.is_null() {
        return;
    }
    let title_str = match unsafe { core::ffi::CStr::from_ptr(title) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return,
    };
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_title(&title_str);
        }
    });
}

/// Return the currently configured title as a NUL-terminated string.
/// The pointer is valid until the next title change.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_title(frame: *mut libdecor_frame) -> *const c_char {
    let Some(b) = (unsafe { FrameBox::as_mut(frame) }) else {
        return core::ptr::null();
    };
    let ctx = unsafe { &mut *b.ctx.as_ptr() };
    let id = b.id;
    let title = ctx
        .rust
        .frame(id)
        .and_then(|f| f.title().ok().flatten().map(|s| s.to_owned()));
    let Some(title) = title else {
        return core::ptr::null();
    };
    let cached = ctx.title_cache.entry(id).or_default();
    let owned = std::ffi::CString::new(title).unwrap_or_default();
    *cached = owned;
    cached.as_ptr()
}

/// Set the application id (xdg-shell `app_id`).
///
/// # Safety
///
/// `app_id` must be a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_app_id(
    frame: *mut libdecor_frame,
    app_id: *const c_char,
) {
    if app_id.is_null() {
        return;
    }
    let app_id_str = match unsafe { core::ffi::CStr::from_ptr(app_id) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return,
    };
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_app_id(&app_id_str);
        }
    });
}

/// Add the given capabilities to the frame's set.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_capabilities(
    frame: *mut libdecor_frame,
    caps: libdecor_capabilities,
) {
    let rcaps = capabilities_from_c(caps);
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_capabilities(rcaps);
        }
    });
}

/// Remove the given capabilities from the frame's set.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_unset_capabilities(
    frame: *mut libdecor_frame,
    caps: libdecor_capabilities,
) {
    let rcaps = capabilities_from_c(caps);
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.unset_capabilities(rcaps);
        }
    });
}

/// Return whether the frame advertises every capability in `caps`.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_has_capability(
    frame: *mut libdecor_frame,
    caps: libdecor_capabilities,
) -> bool {
    let rcaps = capabilities_from_c(caps);
    with_frame_ret(frame, false, |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.has_capability(rcaps).ok())
            .unwrap_or(false)
    })
}

/// Read the frame's currently advertised capabilities as a bitmask.
///
/// # Safety
///
/// `frame` must be a valid frame handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_capabilities(
    frame: *mut libdecor_frame,
) -> libdecor_capabilities {
    with_frame_ret(frame, 0, |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.capabilities().ok())
            .map(capabilities_to_c)
            .unwrap_or(0)
    })
}

/// Show the window menu at the given frame-local coordinate.
///
/// # Safety
///
/// `frame` must be a valid frame handle; `seat` must be a valid
/// `*mut wl_seat`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_show_window_menu(
    frame: *mut libdecor_frame,
    seat: *mut c_void,
    serial: u32,
    x: c_int,
    y: c_int,
) {
    let Some(seat_proxy) = (unsafe { proxy_from_ptr::<WlSeat>(frame, seat) }) else {
        return;
    };
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.show_window_menu(&seat_proxy, serial, x, y);
        }
    });
}

/// Popup-grab tracking is not implemented yet. No-op.
///
/// # Safety
///
/// Pointer arguments are accepted for ABI compatibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_popup_grab(
    _frame: *mut libdecor_frame,
    _seat_name: *const c_char,
) {
}

/// Popup-grab tracking is not implemented yet. No-op.
///
/// # Safety
///
/// Pointer arguments are accepted for ABI compatibility.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_popup_ungrab(
    _frame: *mut libdecor_frame,
    _seat_name: *const c_char,
) {
}

/// Translate surface-local coordinates to frame-local coordinates.
///
/// # Safety
///
/// `frame_x` and `frame_y` must point to writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_translate_coordinate(
    frame: *mut libdecor_frame,
    surface_x: c_int,
    surface_y: c_int,
    frame_x: *mut c_int,
    frame_y: *mut c_int,
) {
    let (fx, fy) = with_frame_ret(frame, (surface_x, surface_y), |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.translate_coordinate(surface_x, surface_y).ok())
            .unwrap_or((surface_x, surface_y))
    });
    if !frame_x.is_null() {
        unsafe { *frame_x = fx };
    }
    if !frame_y.is_null() {
        unsafe { *frame_y = fy };
    }
}

/// Set the minimum content size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_min_content_size(
    frame: *mut libdecor_frame,
    width: c_int,
    height: c_int,
) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_min_content_size(width, height);
        }
    });
}

/// Set the maximum content size. Pass `0` to leave a dimension
/// unbounded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_max_content_size(
    frame: *mut libdecor_frame,
    width: c_int,
    height: c_int,
) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_max_content_size(width, height);
        }
    });
}

/// Read the minimum content size, defaulting to `0,0` when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_min_content_size(
    frame: *mut libdecor_frame,
    width: *mut c_int,
    height: *mut c_int,
) {
    let (w, h) = with_frame_ret(frame, (0, 0), |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.min_content_size().ok().flatten())
            .unwrap_or((0, 0))
    });
    if !width.is_null() {
        unsafe { *width = w };
    }
    if !height.is_null() {
        unsafe { *height = h };
    }
}

/// Read the maximum content size, defaulting to `0,0` when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_max_content_size(
    frame: *mut libdecor_frame,
    width: *mut c_int,
    height: *mut c_int,
) {
    let (w, h) = with_frame_ret(frame, (0, 0), |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.max_content_size().ok().flatten())
            .unwrap_or((0, 0))
    });
    if !width.is_null() {
        unsafe { *width = w };
    }
    if !height.is_null() {
        unsafe { *height = h };
    }
}

/// Start an interactive resize on the given edge.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_resize(
    frame: *mut libdecor_frame,
    seat: *mut c_void,
    serial: u32,
    edge: libdecor_resize_edge,
) {
    let Some(seat_proxy) = (unsafe { proxy_from_ptr::<WlSeat>(frame, seat) }) else {
        return;
    };
    let redge = edge.to_rust();
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.resize(&seat_proxy, serial, redge);
        }
    });
}

/// Start an interactive move.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_move(
    frame: *mut libdecor_frame,
    seat: *mut c_void,
    serial: u32,
) {
    let Some(seat_proxy) = (unsafe { proxy_from_ptr::<WlSeat>(frame, seat) }) else {
        return;
    };
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.r#move(&seat_proxy, serial);
        }
    });
}

/// Commit a new content state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_commit(
    frame: *mut libdecor_frame,
    state: *mut libdecor_state,
    configuration: *mut libdecor_configuration,
) {
    let Some(state_rust) = state_of(state) else {
        return;
    };
    let cfg_ref = unsafe { ConfigurationBox::as_ref(configuration) };
    let cfg_owned = cfg_ref.map(|c| c.rust.clone());
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.commit(&state_rust, cfg_owned.as_ref());
        }
    });
}

/// Request the window to be minimized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_minimized(frame: *mut libdecor_frame) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_minimized();
        }
    });
}

/// Request the window to be maximized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_maximized(frame: *mut libdecor_frame) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_maximized();
        }
    });
}

/// Request the window to leave the maximized state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_unset_maximized(frame: *mut libdecor_frame) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.unset_maximized();
        }
    });
}

/// Request the window to enter fullscreen on the given output (or let
/// the compositor pick if `output` is NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_set_fullscreen(
    frame: *mut libdecor_frame,
    output: *mut c_void,
) {
    let output_proxy = unsafe { proxy_from_ptr::<WlOutput>(frame, output) };
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.set_fullscreen(output_proxy.as_ref());
        }
    });
}

/// Request the window to leave fullscreen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_unset_fullscreen(frame: *mut libdecor_frame) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.unset_fullscreen();
        }
    });
}

/// Return whether the frame is currently in a floating state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_is_floating(frame: *mut libdecor_frame) -> bool {
    with_frame_ret(frame, false, |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.is_floating().ok())
            .unwrap_or(false)
    })
}

/// Close the window. The frame handle remains valid until the C caller
/// drops the last reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_close(frame: *mut libdecor_frame) {
    let Some(b) = (unsafe { FrameBox::as_mut(frame) }) else {
        return;
    };
    let ctx = unsafe { &mut *b.ctx.as_ptr() };
    let id = b.id;
    ctx.frames.remove(&id);
    let _ = ctx.rust.destroy_frame(id);
}

/// Map the frame, triggering the initial configure cycle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_map(frame: *mut libdecor_frame) {
    with_frame(frame, |ctx, id| {
        if let Some(mut f) = ctx.rust.frame(id) {
            let _ = f.map();
        }
    });
}

/// Return the underlying `xdg_surface` pointer for the frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_xdg_surface(frame: *mut libdecor_frame) -> *mut c_void {
    with_frame_ret(frame, core::ptr::null_mut(), |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.xdg_surface().ok().map(XdgSurface::id))
            .map(|id| id.as_ptr().cast::<c_void>())
            .unwrap_or(core::ptr::null_mut())
    })
}

/// Return the underlying `xdg_toplevel` pointer for the frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_xdg_toplevel(
    frame: *mut libdecor_frame,
) -> *mut c_void {
    with_frame_ret(frame, core::ptr::null_mut(), |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.xdg_toplevel().ok().map(XdgToplevel::id))
            .map(|id| id.as_ptr().cast::<c_void>())
            .unwrap_or(core::ptr::null_mut())
    })
}

/// Return the compositor-advertised window manager capabilities.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn libdecor_frame_get_wm_capabilities(
    frame: *mut libdecor_frame,
) -> libdecor_wm_capabilities {
    with_frame_ret(frame, 0, |ctx, id| {
        ctx.rust
            .frame(id)
            .and_then(|f| f.wm_capabilities().ok())
            .map(wm_capabilities_to_c)
            .unwrap_or(0)
    })
}

unsafe fn proxy_from_ptr<P: Proxy>(frame: *mut libdecor_frame, ptr: *mut c_void) -> Option<P> {
    if ptr.is_null() {
        return None;
    }
    let b = unsafe { FrameBox::as_mut(frame) }?;
    let ctx = unsafe { &*b.ctx.as_ptr() };
    let id = unsafe { ObjectId::from_ptr(P::interface(), ptr.cast()) }.ok()?;
    P::from_id(ctx.rust.connection(), id).ok()
}

fn with_frame<F>(frame: *mut libdecor_frame, body: F)
where
    F: FnOnce(&mut ContextBox, libdecor_rs::FrameId),
{
    if let Some(b) = unsafe { FrameBox::as_mut(frame) } {
        let id = b.id;
        let ctx = unsafe { &mut *b.ctx.as_ptr() };
        body(ctx, id);
    }
}

fn with_frame_ret<F, T>(frame: *mut libdecor_frame, fallback: T, body: F) -> T
where
    F: FnOnce(&mut ContextBox, libdecor_rs::FrameId) -> T,
{
    if let Some(b) = unsafe { FrameBox::as_mut(frame) } {
        let id = b.id;
        let ctx = unsafe { &mut *b.ctx.as_ptr() };
        body(ctx, id)
    } else {
        fallback
    }
}

/// Dispatch a Configure event to the given frame's interface.
///
/// # Safety
///
/// `frame` must be the [`NonNull`] returned by [`libdecor_decorate`].
/// `cfg` must be a valid configuration pointer for the duration of the
/// call.
pub(crate) unsafe fn invoke_configure(frame: NonNull<FrameBox>, cfg: *mut libdecor_configuration) {
    let frame_ref = unsafe { frame.as_ref() };
    let iface = unsafe { frame_ref.iface.as_ref() };
    let user_data = frame_ref.user_data;
    if let Some(cb) = iface.configure {
        unsafe { cb(frame.as_ptr().cast(), cfg, user_data) };
    }
}

/// # Safety
///
/// `frame` must be the [`NonNull`] returned by [`libdecor_decorate`].
pub(crate) unsafe fn invoke_close(frame: NonNull<FrameBox>) {
    let frame_ref = unsafe { frame.as_ref() };
    let iface = unsafe { frame_ref.iface.as_ref() };
    let user_data = frame_ref.user_data;
    if let Some(cb) = iface.close {
        unsafe { cb(frame.as_ptr().cast(), user_data) };
    }
}
