//! Top-level [`Context`] type.
//!
//! A `Context` owns the Wayland connection, the event queue, and all
//! frames created from it. Drive it with [`Context::dispatch`] in a loop
//! and consume frame events with [`Context::poll_event`].

use std::os::fd::{AsFd, BorrowedFd};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::globals::registry_queue_init;
use wayland_client::{Connection, EventQueue};
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1;

use crate::error::{Error, Result};
use crate::event::Event;
use crate::frame::Frame;
use crate::id::FrameId;
use crate::inner::{
    FrameKey, FrameSlot, Inner, PendingConfigure, bind, register_surface, unregister_surface,
};
use crate::input::{DecorationPart, SurfaceTarget};
use crate::state::{Capabilities, WindowState, WmCapabilities};

/// libdecor context: a Wayland connection plus the bookkeeping required
/// to decorate one or more toplevel windows.
pub struct Context {
    pub(crate) conn: Connection,
    pub(crate) queue: EventQueue<Inner>,
    pub(crate) inner: Inner,
}

impl Context {
    /// Connect to the Wayland compositor from the environment
    /// (`$WAYLAND_DISPLAY`) and prepare the necessary globals.
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        Self::from_connection(conn)
    }

    /// Build a context from an already-established Wayland connection.
    pub fn from_connection(conn: Connection) -> Result<Self> {
        let (globals, queue) = registry_queue_init::<Inner>(&conn)?;
        let inner = bind(&globals, queue.handle())?;
        Ok(Self { conn, queue, inner })
    }

    /// Build a context that wraps an existing `*mut wl_display` owned by
    /// another library (typically the application linking against
    /// libdecor's C ABI).
    ///
    /// Requires the `system` Cargo feature. The caller is responsible
    /// for keeping the display pointer valid for the lifetime of the
    /// returned [`Context`].
    ///
    /// # Safety
    ///
    /// `display` must be a valid `*mut wl_display` obtained from
    /// libwayland-client. The display must not be disconnected while
    /// this context is alive.
    #[cfg(feature = "system")]
    pub unsafe fn from_display(display: *mut std::ffi::c_void) -> Result<Self> {
        let backend =
            unsafe { wayland_client::backend::Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        Self::from_connection(conn)
    }

    /// Whether the compositor advertised `zxdg_decoration_manager_v1`.
    pub fn supports_server_side_decorations(&self) -> bool {
        self.inner.decoration_mgr.is_some()
    }

    /// Force libdecor to draw its own client-side decorations even when
    /// the compositor claims to support server-side decorations.
    ///
    /// Useful for tiling Wayland compositors that advertise
    /// `xdg-decoration` but do not actually draw decorations, leaving
    /// the window naked otherwise. The environment variable
    /// `LIBDECOR_FORCE_CSD` (any value) sets this at startup.
    ///
    /// This must be called before [`Self::create_frame`] for the
    /// setting to take effect on that frame.
    pub fn force_client_side_decorations(&mut self, force: bool) {
        self.inner.force_csd = force;
    }

    /// Whether libdecor is currently forcing client-side decorations.
    pub fn is_forcing_client_side_decorations(&self) -> bool {
        self.inner.force_csd
    }

    /// Iterate over the Wayland seats currently known to libdecor.
    pub fn seats(&self) -> impl Iterator<Item = &wayland_client::protocol::wl_seat::WlSeat> {
        self.inner.seat_proxies()
    }

    /// Returns the underlying [`wayland_client::Connection`]. Useful for
    /// applications that need to create additional event queues or
    /// allocate Wayland objects outside of libdecor's API surface.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Returns the libdecor [`Dispatcher`](crate::Dispatcher) queue
    /// handle. Pass this to proxy-creation requests (`create_pool`,
    /// `create_buffer`, ...) to route their events through libdecor's
    /// dispatcher.
    pub fn queue_handle(&self) -> &wayland_client::QueueHandle<Inner> {
        &self.inner.qh
    }

    /// The `wl_shm` global bound by libdecor at startup.
    pub fn wl_shm(&self) -> &wayland_client::protocol::wl_shm::WlShm {
        &self.inner.shm
    }

    /// The `wl_compositor` global bound by libdecor at startup.
    pub fn wl_compositor(&self) -> &wayland_client::protocol::wl_compositor::WlCompositor {
        &self.inner.compositor
    }

    /// Dispatch pending Wayland events, blocking until at least one is
    /// available or `timeout` elapses. Pass `None` to block indefinitely.
    ///
    /// Returns the number of events that were dispatched.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<usize> {
        self.queue.flush()?;
        let dispatched = self.queue.dispatch_pending(&mut self.inner)?;
        if dispatched > 0 {
            return Ok(dispatched);
        }

        let read_guard = match self.queue.prepare_read() {
            Some(guard) => guard,
            None => return Ok(self.queue.dispatch_pending(&mut self.inner)?),
        };

        let fd = read_guard.connection_fd();
        let ts = timeout.map(|d| Timespec {
            tv_sec: d.as_secs() as _,
            tv_nsec: d.subsec_nanos() as _,
        });

        let mut pfd = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut pfd, ts.as_ref()) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {
                drop(read_guard);
                return Ok(0);
            }
            Err(e) => return Err(e.into()),
        }

        if pfd[0].revents().contains(PollFlags::IN) {
            read_guard.read()?;
        } else {
            drop(read_guard);
            return Ok(0);
        }
        Ok(self.queue.dispatch_pending(&mut self.inner)?)
    }

    /// Flush queued requests to the compositor without dispatching any
    /// incoming events.
    pub fn flush(&self) -> Result<()> {
        self.conn.flush()?;
        Ok(())
    }

    /// Pull the next pending frame event, if any.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.inner.events.pop_front()
    }

    /// Create a new decorated toplevel window.
    ///
    /// The returned [`FrameId`] can be passed to [`Self::frame`] to
    /// further configure or interact with the window. The window will
    /// not actually appear until [`Frame::map`] is called.
    pub fn create_frame(&mut self) -> Result<FrameId> {
        let wl_surface = self
            .inner
            .compositor
            .create_surface(&self.inner.qh.clone(), ());
        self.decorate_inner(wl_surface, true)
    }

    /// Decorate a Wayland surface that the application already owns.
    ///
    /// Use this when libdecor needs to wrap a `wl_surface` created by
    /// another component (for example, a C application calling through
    /// the FFI shim). The application retains ownership of the surface;
    /// libdecor will not destroy it when the frame is freed.
    pub fn decorate(
        &mut self,
        wl_surface: wayland_client::protocol::wl_surface::WlSurface,
    ) -> Result<FrameId> {
        self.decorate_inner(wl_surface, false)
    }

    fn decorate_inner(
        &mut self,
        wl_surface: wayland_client::protocol::wl_surface::WlSurface,
        owns_wl_surface: bool,
    ) -> Result<FrameId> {
        let id = self.inner.allocate_frame_id();
        let qh = self.inner.qh.clone();

        let xdg_surface = self
            .inner
            .wm_base
            .get_xdg_surface(&wl_surface, &qh, FrameKey(id));
        let xdg_toplevel = xdg_surface.get_toplevel(&qh, FrameKey(id));

        let decoration = if self.inner.force_csd {
            None
        } else {
            self.inner.decoration_mgr.as_ref().map(|mgr| {
                let dec = mgr.get_toplevel_decoration(&xdg_toplevel, &qh, FrameKey(id));
                dec.set_mode(zxdg_toplevel_decoration_v1::Mode::ClientSide);
                dec
            })
        };

        let slot = FrameSlot {
            wl_surface,
            owns_wl_surface,
            xdg_surface,
            xdg_toplevel,
            decoration,
            title: None,
            app_id: None,
            min_size: None,
            max_size: None,
            capabilities: Capabilities::full(),
            wm_capabilities: WmCapabilities::NONE,
            wm_capabilities_known: false,
            window_state: WindowState::NONE,
            content_size: (0, 0),
            decoration_mode: None,
            csd: None,
            visible: true,
            mapped: false,
            pending: PendingConfigure::default(),
        };
        register_surface(
            &mut self.inner,
            &slot.wl_surface,
            SurfaceTarget {
                frame: FrameId(id),
                part: DecorationPart::Content,
            },
        );
        self.inner.frames.insert(id, slot);
        Ok(FrameId(id))
    }

    /// Borrow a previously-created frame for further manipulation.
    pub fn frame(&mut self, id: FrameId) -> Option<Frame<'_>> {
        if self.inner.frames.contains_key(&id.0) {
            Some(Frame { ctx: self, id })
        } else {
            None
        }
    }

    /// Destroy a frame and free the associated Wayland resources.
    pub fn destroy_frame(&mut self, id: FrameId) -> Result<()> {
        let slot = self.inner.frames.remove(&id.0).ok_or(Error::UnknownFrame)?;
        if let Some(csd) = slot.csd {
            unregister_surface(&mut self.inner, &csd.titlebar.wl_surface);
            for border in &csd.borders {
                unregister_surface(&mut self.inner, &border.wl_surface);
            }
            csd.destroy();
        }
        if let Some(dec) = slot.decoration {
            dec.destroy();
        }
        slot.xdg_toplevel.destroy();
        slot.xdg_surface.destroy();
        unregister_surface(&mut self.inner, &slot.wl_surface);
        if slot.owns_wl_surface {
            slot.wl_surface.destroy();
        }
        Ok(())
    }
}

impl AsFd for Context {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.conn.as_fd()
    }
}
