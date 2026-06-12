//! Default-queue event pump.
//!
//! libdecor must deliver `Configure`/`Close` to the application's frame
//! callbacks on the application's own thread. Upstream libdecor gets this
//! for free because its Wayland objects live on the application's
//! *default* event queue, so the application's regular
//! `wl_display_dispatch_pending()` / `wl_display_dispatch()` drive them.
//! `wayland-client`'s system backend forces our objects onto a private
//! queue instead. As the mesa `eglut` loop demonstrates, an application
//! may never call [`libdecor_dispatch`](crate::libdecor_dispatch) at all
//! (its call is guarded by a flag it never sets).
//!
//! To deliver regardless, we mirror upstream's reliance on the default
//! queue: we register a self-re-arming `wl_display.sync` callback **on
//! the default queue** using raw libwayland. This bypasses the system
//! backend's thread-local dispatcher, which would otherwise panic when
//! libwayland (rather than Rust) dispatches it. Its `done` handler runs
//! on the application's thread inside the application's own dispatch, and
//! there we drain and deliver libdecor's queued events. The background
//! [`ContextHandle`](crate::dispatch) thread still services the private
//! queue (input); the pump only delivers.
//!
//! # Teardown ordering
//!
//! The `done` handler may invoke the application's close callback, which
//! (as in `eglut`) calls [`libdecor_unref`](crate::libdecor_unref) and
//! then disconnects the display. [`detach`] therefore stops the worker
//! thread and drops the Wayland-owning [`Context`] *synchronously* while
//! the display is still valid, but keeps this small [`PumpState`] alive
//! (it is the live callback's `user_data`) until the handler unwinds.

use core::ffi::c_void;

use libdecor_rs::Event;
use libdecor_rs::wayland_client::Proxy;
use libdecor_rs::wayland_client::protocol::wl_callback::WlCallback;
use wayland_sys::client::{wayland_client_handle, wl_display, wl_proxy};
use wayland_sys::common::{wl_argument, wl_interface};
use wayland_sys::ffi_dispatch;

use crate::frame::{invoke_close, invoke_configure};
use crate::types::{ConfigurationBox, ContextBox};

/// `wl_display.sync` request opcode.
const WL_DISPLAY_SYNC: u32 = 0;

/// Stable, separately-heap-allocated state used as the sync callback's
/// `user_data`. It outlives the [`ContextBox`] so the in-flight callback
/// can safely observe that the context was torn down mid-delivery.
pub(crate) struct PumpState {
    /// The owning context, or null once it has been torn down.
    ctx: *mut ContextBox,
    /// Raw `wl_display` the sync callbacks are created against.
    display: *mut wl_display,
    /// The currently-armed callback, or null while none is outstanding.
    cb: *mut wl_proxy,
    /// The previous callback, destroyed one dispatch later. Destroying a
    /// `wl_callback` from inside its *own* `done` handler corrupts
    /// libwayland's heap, so we always defer it by a frame, when the proxy
    /// is no longer being dispatched.
    pending_destroy: *mut wl_proxy,
    /// True while we are inside [`sync_done`].
    in_dispatch: bool,
}

static SYNC_LISTENER: [unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32); 1] = [sync_done];

/// Allocate the pump and arm its first callback. The returned pointer is
/// stored in the [`ContextBox`]; it is freed by [`detach`] or
/// [`sync_done`], never by dropping the box.
pub(crate) unsafe fn create(ctx: *mut ContextBox, display: *mut c_void) -> *mut PumpState {
    let pump = Box::into_raw(Box::new(PumpState {
        ctx,
        display: display.cast(),
        cb: core::ptr::null_mut(),
        pending_destroy: core::ptr::null_mut(),
        in_dispatch: false,
    }));
    // Arm under the lock so the raw libwayland call is serialized with the
    // worker thread's dispatch (see `arm`).
    unsafe { (*ctx).rust.with(|_| arm(pump)) };
    pump
}

/// Issue a fresh `wl_display.sync` on the default queue and listen for it.
///
/// The caller must hold the context lock. The worker thread holds the same
/// lock while it dispatches the private queue, so this keeps our raw
/// libwayland proxy churn from running concurrently with the worker's,
/// which corrupts libwayland's object map.
unsafe fn arm(pump: *mut PumpState) {
    let display = unsafe { (*pump).display };
    if display.is_null() {
        return;
    }
    let Some(iface) = WlCallback::interface().c_ptr else {
        return;
    };
    let iface = iface as *const wl_interface;
    // The lone argument is the new-id placeholder; libwayland fills it in.
    let mut args = [wl_argument { n: 0 }];
    let cb = unsafe {
        ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_marshal_array_constructor,
            display as *mut wl_proxy,
            WL_DISPLAY_SYNC,
            args.as_mut_ptr(),
            iface
        )
    };
    if cb.is_null() {
        return;
    }
    unsafe {
        ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_add_listener,
            cb,
            SYNC_LISTENER.as_ptr() as *mut _,
            pump.cast::<c_void>()
        );
        (*pump).cb = cb;
    }
}

/// `wl_callback.done` handler. Runs on the application's thread.
unsafe extern "C" fn sync_done(data: *mut c_void, cb: *mut wl_proxy, _serial: u32) {
    let pump = data as *mut PumpState;

    let ctx = unsafe { (*pump).ctx };
    if ctx.is_null() {
        // The context was already torn down. The current `cb` is being
        // dispatched, so it is left to leak; the display is going away
        // regardless. Free the pump.
        drop(unsafe { Box::from_raw(pump) });
        return;
    }

    unsafe { (*pump).in_dispatch = true };

    // Phase 1, under the lock: hand the previous frame's spent callback to
    // the worker for destruction (we must not destroy default-queue proxies
    // here, inside the application's own dispatch of that queue), drain
    // events, and re-arm. Holding the lock serializes the re-arm with the
    // worker. The current `cb` is deferred one frame so libwayland is fully
    // done with it before the worker destroys it.
    let events = unsafe {
        (*ctx).rust.with(|c| {
            let prev = (*pump).pending_destroy;
            (*ctx).rust.queue_destroy(prev);
            (*pump).pending_destroy = cb;
            (*pump).cb = core::ptr::null_mut();
            let mut events = Vec::new();
            while let Some(ev) = c.poll_event() {
                events.push(ev);
            }
            arm(pump);
            events
        })
    };

    // Phase 2, without the lock: deliver to the application's callbacks,
    // which may re-enter the C ABI (and tear the context down on close).
    unsafe { deliver_events(&mut *ctx, events) };
    unsafe { (*pump).in_dispatch = false };

    if unsafe { (*pump).ctx }.is_null() {
        // Delivery tore the context down (the close callback). The worker is
        // stopped; release the pump. The callback armed above leaks, but the
        // display is being disconnected.
        drop(unsafe { Box::from_raw(pump) });
    }
}

/// Detach the pump from a context that is being freed. Called from
/// [`libdecor_unref`](crate::libdecor_unref).
pub(crate) unsafe fn detach(boxed: &mut ContextBox) {
    let pump = boxed.pump;
    boxed.pump = core::ptr::null_mut();
    if pump.is_null() {
        return;
    }
    unsafe { (*pump).ctx = core::ptr::null_mut() };
    if unsafe { (*pump).in_dispatch } {
        // We are unwinding through sync_done; it owns the pump now and
        // will free it.
        return;
    }
    // Normal teardown: destroy the armed callback and the deferred one,
    // then free the pump. Neither is currently being dispatched.
    let cb = unsafe { (*pump).cb };
    if !cb.is_null() {
        unsafe { ffi_dispatch!(wayland_client_handle(), wl_proxy_destroy, cb) };
    }
    let prev = unsafe { (*pump).pending_destroy };
    if !prev.is_null() {
        unsafe { ffi_dispatch!(wayland_client_handle(), wl_proxy_destroy, prev) };
    }
    drop(unsafe { Box::from_raw(pump) });
}

/// Deliver drained events to the application's frame callbacks.
///
/// Stops at the first `Close`: that callback may free `boxed`, so nothing
/// after it may touch the context.
pub(crate) unsafe fn deliver_events(boxed: &mut ContextBox, events: Vec<Event>) -> i32 {
    let mut dispatched: i32 = 0;
    for event in events {
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
                // The close callback may have destroyed the context.
                return dispatched;
            }
            Event::Commit { .. } | Event::DismissPopup { .. } | Event::Bounds { .. } => {}
        }
    }
    dispatched
}
