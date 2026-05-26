//! Private state shared by the Wayland event queue dispatchers.
//!
//! All `Dispatch` implementations live here. They mutate frame slots in
//! place and push high-level [`Event`]s onto a queue that the public
//! [`Context`](crate::Context) drains.

use std::collections::{HashMap, VecDeque};

use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_callback::WlCallback,
    wl_compositor::WlCompositor,
    wl_keyboard::WlKeyboard,
    wl_pointer::{self, WlPointer},
    wl_registry::WlRegistry,
    wl_seat::{self, WlSeat},
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_surface::WlSurface,
    wl_touch::WlTouch,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
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
use crate::csd::{BORDER_WIDTH, Decoration, TITLEBAR_HEIGHT};
use crate::event::Event;
use crate::id::FrameId;
use crate::input::{BorderEdge, DecorationPart, PointerFocus, SeatState, SurfaceTarget};
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
    /// `true` after the compositor has delivered an
    /// `xdg_toplevel::wm_capabilities` event. When `false`, libdecor
    /// assumes the compositor does not yet implement the protocol and
    /// shows all buttons.
    pub(crate) wm_capabilities_known: bool,
    pub(crate) window_state: WindowState,
    pub(crate) content_size: (i32, i32),

    pub(crate) decoration_mode: Option<zxdg_toplevel_decoration_v1::Mode>,

    /// Client-side decoration state, populated when libdecor draws the
    /// titlebar itself.
    pub(crate) csd: Option<Decoration>,

    pub(crate) visible: bool,
    pub(crate) mapped: bool,

    pub(crate) pending: PendingConfigure,
}

impl FrameSlot {
    /// Decoration overhead in each direction `(top, bottom, left, right)`
    /// in surface-local pixels. Zero in every dimension when no CSD is
    /// active.
    pub(crate) fn decoration_overhead(&self) -> (i32, i32, i32, i32) {
        if self.csd.is_some() {
            (
                TITLEBAR_HEIGHT + BORDER_WIDTH,
                BORDER_WIDTH,
                BORDER_WIDTH,
                BORDER_WIDTH,
            )
        } else {
            (0, 0, 0, 0)
        }
    }

    /// Vertical decoration above the content surface (titlebar + top
    /// border, when CSD is active).
    pub(crate) fn top_decoration(&self) -> i32 {
        self.decoration_overhead().0
    }
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
    #[allow(dead_code)]
    pub(crate) subcompositor: Option<WlSubcompositor>,
    #[allow(dead_code)]
    pub(crate) shm: WlShm,
    pub(crate) wm_base: XdgWmBase,
    pub(crate) decoration_mgr: Option<ZxdgDecorationManagerV1>,
    pub(crate) seats: HashMap<ObjectId, SeatHandle>,

    /// When `true`, libdecor will not request `xdg-decoration` mode at
    /// all and will always draw its own CSD. Useful on tiling
    /// compositors that advertise the decoration protocol but do not
    /// actually draw server-side decorations.
    pub(crate) force_csd: bool,

    pub(crate) frames: HashMap<usize, FrameSlot>,
    pub(crate) next_id: usize,

    /// Lookup from a `wl_surface` id to the frame and decoration part it
    /// represents. Populated when libdecor creates frames or
    /// decoration subsurfaces.
    pub(crate) surface_targets: HashMap<ObjectId, SurfaceTarget>,

    /// Current pointer focus per pointer object id.
    pub(crate) pointer_focus: HashMap<ObjectId, PointerFocus>,

    pub(crate) events: VecDeque<Event>,
}

/// libdecor's view of a `wl_seat`: the seat itself plus the input
/// devices we created from it.
pub(crate) struct SeatHandle {
    pub(crate) seat: WlSeat,
    pub(crate) input: SeatState,
}

impl Inner {
    pub(crate) fn allocate_frame_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Return the list of seats currently known.
    pub(crate) fn seat_proxies(&self) -> impl Iterator<Item = &WlSeat> {
        self.seats.values().map(|h| &h.seat)
    }
}

impl Dispatch<WlRegistry, wayland_client::globals::GlobalListContents> for Inner {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
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
        ensure_csd(state, id);

        let configuration = {
            let Some(slot) = state.frames.get_mut(&key.0) else {
                return;
            };
            let pending = std::mem::take(&mut slot.pending);
            let (top, bottom, left, right) = slot.decoration_overhead();
            let size_adjusted = pending
                .size
                .map(|(w, h)| ((w - left - right).max(1), (h - top - bottom).max(1)));
            Configuration {
                serial,
                size: size_adjusted,
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
                slot.wm_capabilities_known = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgDecorationManagerV1, ()> for Inner {
    fn event(
        _: &mut Self,
        _: &ZxdgDecorationManagerV1,
        _: <ZxdgDecorationManagerV1 as Proxy>::Event,
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
        {
            if let Some(slot) = state.frames.get_mut(&key.0) {
                slot.decoration_mode = Some(mode);
            }
            ensure_csd(state, FrameId(key.0));
        }
    }
}

impl Dispatch<WlSeat, ()> for Inner {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let id = seat.id();
            let handle = state.seats.entry(id).or_insert_with(|| SeatHandle {
                seat: seat.clone(),
                input: SeatState::new(),
            });

            let has_pointer = caps.contains(wl_seat::Capability::Pointer);
            if has_pointer && handle.input.pointer.is_none() {
                handle.input.pointer = Some(seat.get_pointer(qh, ()));
            } else if !has_pointer
                && let Some(p) = handle.input.pointer.take()
            {
                p.release();
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for Inner {
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let pointer_id = pointer.id();
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                let surface_id = surface.id();
                if let Some(target) = state.surface_targets.get(&surface_id).copied() {
                    state.pointer_focus.insert(
                        pointer_id.clone(),
                        PointerFocus {
                            target,
                            surface_id,
                            serial,
                            x: surface_x,
                            y: surface_y,
                            button_down: false,
                        },
                    );
                    if target.part == DecorationPart::Titlebar {
                        update_titlebar_hover(state, target.frame, surface_x, surface_y);
                    }
                }
            }
            wl_pointer::Event::Leave { surface, .. } => {
                let removed = if let Some(focus) = state.pointer_focus.get(&pointer_id) {
                    if focus.surface_id == surface.id() {
                        Some(focus.target)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(target) = removed {
                    state.pointer_focus.remove(&pointer_id);
                    if target.part == DecorationPart::Titlebar
                        && let Some(slot) = state.frames.get_mut(&target.frame.0)
                    {
                        let (cw, ch) = slot.content_size;
                        let title = slot.title.clone();
                        if let Some(dec) = slot.csd.as_mut() {
                            dec.hover = None;
                            dec.pressed = None;
                            let _ = dec.render(&state.shm, &state.qh, cw, ch, title.as_deref());
                        }
                    }
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let target = state.pointer_focus.get_mut(&pointer_id).map(|focus| {
                    focus.x = surface_x;
                    focus.y = surface_y;
                    focus.target
                });
                if let Some(target) = target
                    && target.part == DecorationPart::Titlebar
                {
                    update_titlebar_hover(state, target.frame, surface_x, surface_y);
                }
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                serial,
                ..
            } => {
                if button != 0x110 {
                    return;
                }
                let pressed =
                    matches!(button_state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                let (target, x, y) = match state.pointer_focus.get_mut(&pointer_id) {
                    Some(f) => {
                        f.serial = serial;
                        f.button_down = pressed;
                        (f.target, f.x, f.y)
                    }
                    None => return,
                };
                match target.part {
                    DecorationPart::Titlebar => {
                        if pressed {
                            handle_titlebar_press(state, target.frame, &pointer_id, x, y, serial);
                        } else {
                            handle_titlebar_release(state, target.frame, x, y);
                        }
                    }
                    DecorationPart::Border(edge) => {
                        if pressed {
                            handle_border_press(state, target.frame, &pointer_id, edge, serial);
                        }
                    }
                    DecorationPart::Content => {}
                }
            }
            _ => {}
        }
    }
}

fn update_titlebar_hover(state: &mut Inner, frame_id: FrameId, x: f64, y: f64) {
    let Some(slot) = state.frames.get_mut(&frame_id.0) else {
        return;
    };
    let (content_w, content_h) = slot.content_size;
    let title = slot.title.clone();
    let Some(dec) = slot.csd.as_mut() else {
        return;
    };
    let new_hover = dec.hit_test(x, y);
    if new_hover != dec.hover {
        dec.hover = new_hover;
        let _ = dec.render(
            &state.shm,
            &state.qh,
            content_w,
            content_h,
            title.as_deref(),
        );
    }
}

fn handle_titlebar_press(
    state: &mut Inner,
    frame_id: FrameId,
    pointer_id: &ObjectId,
    x: f64,
    y: f64,
    serial: u32,
) {
    let action_button = {
        let Some(slot) = state.frames.get_mut(&frame_id.0) else {
            return;
        };
        let (content_w, content_h) = slot.content_size;
        let title = slot.title.clone();
        let Some(dec) = slot.csd.as_mut() else {
            return;
        };
        let hit = dec.hit_test(x, y);
        dec.pressed = hit;
        let _ = dec.render(
            &state.shm,
            &state.qh,
            content_w,
            content_h,
            title.as_deref(),
        );
        hit
    };

    if action_button.is_some() {
        return;
    }

    let seat = state
        .seats
        .values()
        .find(|h| {
            h.input
                .pointer
                .as_ref()
                .map(|p| p.id() == *pointer_id)
                .unwrap_or(false)
        })
        .map(|h| h.seat.clone());
    if let (Some(seat), Some(slot)) = (seat, state.frames.get(&frame_id.0)) {
        slot.xdg_toplevel._move(&seat, serial);
    }
}

fn handle_titlebar_release(state: &mut Inner, frame_id: FrameId, x: f64, y: f64) {
    let Some(slot) = state.frames.get_mut(&frame_id.0) else {
        return;
    };
    let (content_w, content_h) = slot.content_size;
    let title = slot.title.clone();
    let Some(dec) = slot.csd.as_mut() else {
        return;
    };
    let released_on = dec.hit_test(x, y);
    let pressed = dec.pressed.take();
    let _ = dec.render(
        &state.shm,
        &state.qh,
        content_w,
        content_h,
        title.as_deref(),
    );
    if let (Some(p), Some(r)) = (pressed, released_on)
        && p == r
    {
        use crate::csd::ButtonKind;
        match p {
            ButtonKind::Close => {
                state.events.push_back(Event::Close { frame: frame_id });
            }
            ButtonKind::Maximize => {
                if slot.window_state.contains(WindowState::MAXIMIZED) {
                    slot.xdg_toplevel.unset_maximized();
                } else {
                    slot.xdg_toplevel.set_maximized();
                }
            }
            ButtonKind::Minimize => {
                slot.xdg_toplevel.set_minimized();
            }
        }
    }
}

fn handle_border_press(
    state: &mut Inner,
    frame_id: FrameId,
    pointer_id: &ObjectId,
    edge: BorderEdge,
    serial: u32,
) {
    let seat = state
        .seats
        .values()
        .find(|h| {
            h.input
                .pointer
                .as_ref()
                .map(|p| p.id() == *pointer_id)
                .unwrap_or(false)
        })
        .map(|h| h.seat.clone());
    let Some(seat) = seat else {
        return;
    };
    let Some(slot) = state.frames.get(&frame_id.0) else {
        return;
    };
    let xdg_edge = match edge {
        BorderEdge::Top => xdg_toplevel::ResizeEdge::Top,
        BorderEdge::Bottom => xdg_toplevel::ResizeEdge::Bottom,
        BorderEdge::Left => xdg_toplevel::ResizeEdge::Left,
        BorderEdge::Right => xdg_toplevel::ResizeEdge::Right,
    };
    slot.xdg_toplevel.resize(&seat, serial, xdg_edge);
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
delegate_noop!(Inner: ignore WlSubsurface);
delegate_noop!(Inner: ignore WlShm);
delegate_noop!(Inner: ignore WlShmPool);
delegate_noop!(Inner: ignore WlBuffer);
delegate_noop!(Inner: ignore WlSurface);
delegate_noop!(Inner: ignore WlCallback);
delegate_noop!(Inner: ignore WlKeyboard);
delegate_noop!(Inner: ignore WlTouch);

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

    let mut seats = HashMap::new();
    for global in globals.contents().clone_list() {
        if global.interface == "wl_seat" {
            let seat = globals.registry().bind::<WlSeat, _, _>(
                global.name,
                global.version.min(7),
                &qh,
                (),
            );
            seats.insert(
                seat.id(),
                SeatHandle {
                    seat,
                    input: SeatState::new(),
                },
            );
        }
    }

    let force_csd = std::env::var_os("LIBDECOR_FORCE_CSD").is_some();

    Ok(Inner {
        qh,
        compositor,
        subcompositor,
        shm,
        wm_base,
        decoration_mgr,
        seats,
        force_csd,
        frames: HashMap::new(),
        next_id: 0,
        surface_targets: HashMap::new(),
        pointer_focus: HashMap::new(),
        events: VecDeque::new(),
    })
}

/// Register a `wl_surface` as belonging to a frame and representing a
/// particular decoration part. Used by `Context::create_frame` and by
/// the CSD plugin when allocating subsurfaces.
pub(crate) fn register_surface(inner: &mut Inner, surface: &WlSurface, target: SurfaceTarget) {
    inner.surface_targets.insert(surface.id(), target);
}

/// Unregister a `wl_surface` (used when frames or decoration
/// subsurfaces are destroyed).
pub(crate) fn unregister_surface(inner: &mut Inner, surface: &WlSurface) {
    let id = surface.id();
    inner.surface_targets.remove(&id);
    inner
        .pointer_focus
        .retain(|_, focus| focus.surface_id != id);
}

/// Allocate a [`Decoration`] for `frame_id` if CSD is needed and not yet
/// present. Returns `true` if a decoration was newly created.
pub(crate) fn ensure_csd(state: &mut Inner, frame_id: FrameId) -> bool {
    let key = frame_id.0;
    let force_csd = state.force_csd;
    let needs_csd = match state.frames.get(&key) {
        Some(slot) => {
            slot.csd.is_none()
                && (force_csd
                    || match slot.decoration_mode {
                        Some(zxdg_toplevel_decoration_v1::Mode::ClientSide) => true,
                        Some(zxdg_toplevel_decoration_v1::Mode::ServerSide) => false,
                        None => state.decoration_mgr.is_none(),
                        _ => false,
                    })
        }
        None => false,
    };
    if !needs_csd {
        return false;
    }
    let Some(subcompositor) = state.subcompositor.clone() else {
        return false;
    };
    let parent = state.frames.get(&key).unwrap().wl_surface.clone();
    let dec = Decoration::new(&state.compositor, &subcompositor, &parent, &state.qh);
    let titlebar_id = dec.titlebar_surface_id();
    state.surface_targets.insert(
        titlebar_id,
        SurfaceTarget {
            frame: frame_id,
            part: DecorationPart::Titlebar,
        },
    );
    for edge in [
        BorderEdge::Top,
        BorderEdge::Bottom,
        BorderEdge::Left,
        BorderEdge::Right,
    ] {
        let id = dec.border_surface_id(edge);
        state.surface_targets.insert(
            id,
            SurfaceTarget {
                frame: frame_id,
                part: DecorationPart::Border(edge),
            },
        );
    }
    state.frames.get_mut(&key).unwrap().csd = Some(dec);
    true
}
