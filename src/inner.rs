//! Private state shared by the Wayland event queue dispatchers.
//!
//! All `Dispatch` implementations live here. They mutate frame slots in
//! place and push high-level [`Event`]s onto a queue that the public
//! [`Context`](crate::Context) drains.

use std::collections::{HashMap, VecDeque};

use wayland_client::globals::GlobalList;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_callback::WlCallback, wl_compositor::WlCompositor,
    wl_registry::WlRegistry, wl_seat::WlSeat, wl_shm::WlShm, wl_shm_pool::WlShmPool,
    wl_subcompositor::WlSubcompositor, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

use crate::configuration::Configuration;
use crate::event::Event;
use crate::id::FrameId;
use crate::state::{Capabilities, WindowState, WmCapabilities};

/// User-data attached to per-frame proxies. Carries the frame slot key
/// so dispatchers can route events to the right [`FrameSlot`].
#[derive(Copy, Clone, Debug)]
pub(crate) struct FrameKey(pub(crate) usize);

/// In-flight configure state, accumulated from `xdg_toplevel::configure`
/// and friends until the final `xdg_surface::configure` flushes it.
#[derive(Default)]
pub(crate) struct PendingConfigure {
    pub(crate) size: Option<(i32, i32)>,
    pub(crate) window_state: Option<WindowState>,
    pub(crate) bounds: Option<(i32, i32)>,
}

/// Everything libdecor tracks for a single window.
pub(crate) struct FrameSlot {
    pub(crate) wl_surface: WlSurface,
    pub(crate) xdg_surface: XdgSurface,
    pub(crate) xdg_toplevel: XdgToplevel,
    pub(crate) decoration: Option<ZxdgToplevelDecorationV1>,

    pub(crate) title: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) min_size: Option<(i32, i32)>,
    pub(crate) max_size: Option<(i32, i32)>,

    pub(crate) capabilities: Capabilities,
    pub(crate) wm_capabilities: WmCapabilities,
    pub(crate) window_state: WindowState,
    pub(crate) content_size: (i32, i32),

    pub(crate) decoration_mode: Option<zxdg_toplevel_decoration_v1::Mode>,

    pub(crate) visible: bool,
    pub(crate) mapped: bool,

    pub(crate) pending: PendingConfigure,
}

/// Opaque dispatcher state owned by [`Context`](crate::Context).
///
/// You will rarely use this type directly. Its only public role is to
/// appear as the type parameter of
/// [`wayland_client::QueueHandle`](wayland_client::QueueHandle) returned
/// by [`Context::queue_handle`](crate::Context::queue_handle), so that
/// proxies created by the application share libdecor's event queue.
pub struct Inner {
    pub(crate) qh: QueueHandle<Inner>,
    pub(crate) compositor: WlCompositor,
    /// Held for future client-side decoration subsurfaces.
    #[allow(dead_code)]
    pub(crate) subcompositor: Option<WlSubcompositor>,
    /// Held for future client-side decoration buffer allocation.
    #[allow(dead_code)]
    pub(crate) shm: WlShm,
    pub(crate) wm_base: XdgWmBase,
    pub(crate) decoration_mgr: Option<ZxdgDecorationManagerV1>,
    pub(crate) seats: Vec<WlSeat>,

    pub(crate) frames: HashMap<usize, FrameSlot>,
    pub(crate) next_id: usize,

    pub(crate) events: VecDeque<Event>,
}

impl Inner {
    pub(crate) fn allocate_frame_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

impl Dispatch<WlRegistry, wayland_client::globals::GlobalListContents> for Inner {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for Inner {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, FrameKey> for Inner {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        key: &FrameKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };

        let id = FrameId(key.0);
        let configuration = {
            let Some(slot) = state.frames.get_mut(&key.0) else {
                return;
            };
            let pending = std::mem::take(&mut slot.pending);
            Configuration {
                serial,
                size: pending.size,
                window_state: pending.window_state,
                bounds: pending.bounds,
            }
        };
        state.events.push_back(Event::Configure {
            frame: id,
            configuration,
        });
    }
}

impl Dispatch<XdgToplevel, FrameKey> for Inner {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        key: &FrameKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = FrameId(key.0);
        let Some(slot) = state.frames.get_mut(&key.0) else {
            return;
        };

        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let size = if width > 0 && height > 0 {
                    Some((width, height))
                } else {
                    None
                };
                let ws = decode_states(&states);
                slot.pending.size = size;
                slot.pending.window_state = Some(ws);
            }
            xdg_toplevel::Event::Close => {
                state.events.push_back(Event::Close { frame: id });
            }
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                slot.pending.bounds = Some((width, height));
                state.events.push_back(Event::Bounds {
                    frame: id,
                    width,
                    height,
                });
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                slot.wm_capabilities = decode_wm_capabilities(&capabilities);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for Inner {
    fn event(
        _: &mut Self,
        _: &ZxdgDecorationManagerV1,
        _: <ZxdgDecorationManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, FrameKey> for Inner {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        key: &FrameKey,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(mode),
        } = event
            && let Some(slot) = state.frames.get_mut(&key.0)
        {
            slot.decoration_mode = Some(mode);
        }
    }
}

fn decode_states(raw: &[u8]) -> WindowState {
    let mut out = WindowState::NONE;
    for chunk in raw.chunks_exact(4) {
        let val = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        out |= match xdg_toplevel::State::try_from(val) {
            Ok(xdg_toplevel::State::Maximized) => WindowState::MAXIMIZED,
            Ok(xdg_toplevel::State::Fullscreen) => WindowState::FULLSCREEN,
            Ok(xdg_toplevel::State::Resizing) => WindowState::RESIZING,
            Ok(xdg_toplevel::State::Activated) => WindowState::ACTIVE,
            Ok(xdg_toplevel::State::TiledLeft) => WindowState::TILED_LEFT,
            Ok(xdg_toplevel::State::TiledRight) => WindowState::TILED_RIGHT,
            Ok(xdg_toplevel::State::TiledTop) => WindowState::TILED_TOP,
            Ok(xdg_toplevel::State::TiledBottom) => WindowState::TILED_BOTTOM,
            Ok(xdg_toplevel::State::Suspended) => WindowState::SUSPENDED,
            Ok(xdg_toplevel::State::ConstrainedLeft) => WindowState::CONSTRAINED_LEFT,
            Ok(xdg_toplevel::State::ConstrainedRight) => WindowState::CONSTRAINED_RIGHT,
            Ok(xdg_toplevel::State::ConstrainedTop) => WindowState::CONSTRAINED_TOP,
            Ok(xdg_toplevel::State::ConstrainedBottom) => WindowState::CONSTRAINED_BOTTOM,
            _ => WindowState::NONE,
        };
    }
    out
}

fn decode_wm_capabilities(raw: &[u8]) -> WmCapabilities {
    let mut out = WmCapabilities::NONE;
    for chunk in raw.chunks_exact(4) {
        let val = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        out |= match xdg_toplevel::WmCapabilities::try_from(val) {
            Ok(xdg_toplevel::WmCapabilities::WindowMenu) => WmCapabilities::WINDOW_MENU,
            Ok(xdg_toplevel::WmCapabilities::Maximize) => WmCapabilities::MAXIMIZE,
            Ok(xdg_toplevel::WmCapabilities::Fullscreen) => WmCapabilities::FULLSCREEN,
            Ok(xdg_toplevel::WmCapabilities::Minimize) => WmCapabilities::MINIMIZE,
            _ => WmCapabilities::NONE,
        };
    }
    out
}

delegate_noop!(Inner: ignore WlCompositor);
delegate_noop!(Inner: ignore WlSubcompositor);
delegate_noop!(Inner: ignore WlShm);
delegate_noop!(Inner: ignore WlShmPool);
delegate_noop!(Inner: ignore WlBuffer);
delegate_noop!(Inner: ignore WlSurface);
delegate_noop!(Inner: ignore WlCallback);

impl Dispatch<WlSeat, ()> for Inner {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Bind the globals libdecor cares about. Returns `Ok(Inner)` if the
/// compositor provides at least `wl_compositor`, `wl_shm`, and
/// `xdg_wm_base`; the decoration manager and subcompositor are optional.
pub(crate) fn bind(
    globals: &GlobalList,
    qh: QueueHandle<Inner>,
) -> Result<Inner, crate::error::Error> {
    use crate::error::Error;

    let compositor: WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|_| Error::MissingGlobal("wl_compositor"))?;
    let shm: WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| Error::MissingGlobal("wl_shm"))?;
    let wm_base: XdgWmBase = globals
        .bind(&qh, 1..=6, ())
        .map_err(|_| Error::MissingGlobal("xdg_wm_base"))?;
    let subcompositor: Option<WlSubcompositor> = globals.bind(&qh, 1..=1, ()).ok();
    let decoration_mgr: Option<ZxdgDecorationManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

    let mut seats = Vec::new();
    for global in globals.contents().clone_list() {
        if global.interface == "wl_seat" {
            let seat = globals.registry().bind::<WlSeat, _, _>(
                global.name,
                global.version.min(7),
                &qh,
                (),
            );
            seats.push(seat);
        }
    }

    Ok(Inner {
        qh,
        compositor,
        subcompositor,
        shm,
        wm_base,
        decoration_mgr,
        seats,
        frames: HashMap::new(),
        next_id: 0,
        events: VecDeque::new(),
    })
}
