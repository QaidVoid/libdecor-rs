//! Background dispatcher for libdecor's private Wayland queue.
//!
//! `wayland-client`'s system backend forces libdecor's proxies onto a
//! private `wl_event_queue` that the application never dispatches. A
//! worker thread therefore services that queue so input driven work
//! (hover rendering, interactive move/resize, the buttons) keeps flowing
//! no matter how the application pumps the socket.
//!
//! The worker never reads the socket itself (the application and Mesa do
//! that); it only dispatches events libwayland has already routed to the
//! private queue, on a short adaptive timer. Application-facing events
//! (`Configure`, `Close`) are *not* delivered from the worker. They are
//! delivered on the application's own thread by the default-queue pump in
//! [`crate::pump`], which must be the sole thing dispatching there.

use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use libdecor_rs::{Context, Event};
use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll};
use wayland_sys::client::{wayland_client_handle, wl_proxy};
use wayland_sys::ffi_dispatch;

/// Shortest dispatch interval, used while input is actively flowing so
/// hover/drag feel responsive.
const ACTIVE_INTERVAL: Duration = Duration::from_millis(2);
/// Longest dispatch interval, used once the queue has been idle for a
/// while to keep wakeups (and power draw) low.
const IDLE_INTERVAL: Duration = Duration::from_millis(16);
/// Consecutive empty ticks before backing off to [`IDLE_INTERVAL`].
const IDLE_AFTER: u32 = 8;

/// Owns the libdecor [`Context`] plus the worker that services its
/// private queue.
pub(crate) struct ContextHandle {
    shared: Arc<Mutex<Context>>,
    /// Spent default-queue `wl_callback` proxies awaiting destruction.
    ///
    /// The default-queue pump must not destroy them itself: it runs inside
    /// the application's own dispatch of that queue, and destroying a proxy
    /// there corrupts libwayland. They are handed here and destroyed by the
    /// worker thread instead, off that dispatch. Pointers are stored as
    /// `usize` so the queue is [`Send`].
    garbage: Arc<Mutex<Vec<usize>>>,
    /// Signaled by the main thread to wake the worker (shutdown).
    wake: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ContextHandle {
    pub(crate) fn new(ctx: Context) -> std::io::Result<Self> {
        let flags = EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK;
        let wake = Arc::new(eventfd(0, flags)?);
        let shared = Arc::new(Mutex::new(ctx));
        let garbage = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let shared = Arc::clone(&shared);
            let garbage = Arc::clone(&garbage);
            let wake = Arc::clone(&wake);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("libdecor-dispatch".to_owned())
                .spawn(move || run(&shared, &garbage, &wake, &stop))?
        };

        Ok(Self {
            shared,
            garbage,
            wake,
            stop,
            thread: Some(thread),
        })
    }

    /// Hand a spent `wl_callback` proxy to the worker for destruction.
    pub(crate) fn queue_destroy(&self, proxy: *mut wl_proxy) {
        if !proxy.is_null() {
            self.garbage
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(proxy as usize);
        }
    }

    /// Run `f` with exclusive access to the [`Context`]. Recovers from a
    /// poisoned lock so a panic in one C ABI call cannot wedge the rest.
    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut Context) -> R) -> R {
        let mut ctx = lock(&self.shared);
        f(&mut ctx)
    }

    /// The Wayland socket fd, for applications that poll it the way they
    /// poll upstream libdecor's fd.
    pub(crate) fn socket_fd(&self) -> RawFd {
        lock(&self.shared).as_fd().as_raw_fd()
    }

    /// Stop and join the worker. Idempotent. Must run before the
    /// application disconnects the display, so the worker is not touching
    /// it concurrently.
    pub(crate) fn shutdown(&mut self) {
        if let Some(handle) = self.thread.take() {
            self.stop.store(true, Ordering::Release);
            signal(self.wake.as_fd());
            let _ = handle.join();
        }
    }

    /// Read the socket (blocking up to `timeout`), dispatch, and drain.
    /// Used by [`libdecor_dispatch`], which services the application's
    /// initial `while (!configured)` loop before the pump can fire.
    ///
    /// [`libdecor_dispatch`]: crate::libdecor_dispatch
    pub(crate) fn dispatch_read(&self, timeout: Option<Duration>) -> Vec<Event> {
        let mut ctx = lock(&self.shared);
        let _ = ctx.dispatch(timeout);
        drain(&mut ctx)
    }
}

impl Drop for ContextHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock(shared: &Mutex<Context>) -> MutexGuard<'_, Context> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn drain(ctx: &mut Context) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(ev) = ctx.poll_event() {
        out.push(ev);
    }
    out
}

/// Worker loop. Dispatches the private queue on an adaptive timer. Never
/// reads the socket, so it does not contend with Mesa's reader.
fn run(shared: &Mutex<Context>, garbage: &Mutex<Vec<usize>>, wake: &OwnedFd, stop: &AtomicBool) {
    let mut idle_ticks: u32 = 0;
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        let dispatched = {
            let mut ctx = lock(shared);
            let n = match ctx.dispatch_pending() {
                Ok(n) => n,
                Err(_) => return,
            };
            // Push out any requests the handlers just queued (cursor shape,
            // decoration repaints, interactive move/resize).
            let _ = ctx.flush();
            // Destroy spent pump callbacks here, holding the context lock so
            // this is serialized with the pump's creation of new ones, and
            // off the application's own dispatch of the default queue.
            collect_garbage(garbage);
            n
        };

        idle_ticks = if dispatched > 0 {
            0
        } else {
            idle_ticks.saturating_add(1)
        };
        let interval = if idle_ticks < IDLE_AFTER {
            ACTIVE_INTERVAL
        } else {
            IDLE_INTERVAL
        };

        let mut fds = [PollFd::new(wake, PollFlags::IN)];
        let ts = Timespec {
            tv_sec: interval.as_secs() as _,
            tv_nsec: interval.subsec_nanos() as _,
        };
        match poll(&mut fds, Some(&ts)) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(_) => return,
        }
        if fds[0].revents().contains(PollFlags::IN) {
            drain_wake(wake.as_fd());
        }
    }
}

/// Destroy every proxy queued for destruction.
fn collect_garbage(garbage: &Mutex<Vec<usize>>) {
    let spent: Vec<usize> = std::mem::take(&mut garbage.lock().unwrap_or_else(|e| e.into_inner()));
    for proxy in spent {
        unsafe {
            ffi_dispatch!(
                wayland_client_handle(),
                wl_proxy_destroy,
                proxy as *mut wl_proxy
            );
        }
    }
}

fn signal(fd: std::os::fd::BorrowedFd<'_>) {
    let _ = rustix::io::write(fd, &1u64.to_ne_bytes());
}

fn drain_wake(fd: std::os::fd::BorrowedFd<'_>) {
    let mut buf = [0u8; 8];
    let _ = rustix::io::read(fd, &mut buf);
}
