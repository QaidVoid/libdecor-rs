//! Per-window frame handle.
//!
//! Use [`Context::frame`](crate::Context::frame) to obtain a temporary
//! [`Frame`] reference for manipulating a window identified by a
//! [`FrameId`](crate::FrameId).

use wayland_client::protocol::{wl_seat::WlSeat, wl_surface::WlSurface};
use wayland_protocols::xdg::shell::client::{xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel};

use crate::configuration::Configuration;
use crate::context::Context;
use crate::csd::ButtonSet;
use crate::error::{Error, Result};
use crate::id::FrameId;
use crate::state::{Capabilities, ResizeEdge, State, WindowState, WmCapabilities};

fn compute_buttons(capabilities: Capabilities, wm: WmCapabilities, wm_known: bool) -> ButtonSet {
    let close = capabilities.contains(Capabilities::CLOSE);
    let maximize = capabilities.contains(Capabilities::FULLSCREEN)
        && (!wm_known || wm.contains(WmCapabilities::MAXIMIZE));
    let minimize = capabilities.contains(Capabilities::MINIMIZE)
        && (!wm_known || wm.contains(WmCapabilities::MINIMIZE));
    ButtonSet {
        close,
        maximize,
        minimize,
    }
}

/// Borrowed reference to a frame, returned by
/// [`Context::frame`](crate::Context::frame).
///
/// All frame-manipulation methods live here. The reference borrows the
/// owning [`Context`] mutably, so only one frame can be addressed at a
/// time.
pub struct Frame<'a> {
    pub(crate) ctx: &'a mut Context,
    pub(crate) id: FrameId,
}

impl<'a> Frame<'a> {
    /// Returns this frame's id.
    pub fn id(&self) -> FrameId {
        self.id
    }

    fn slot_mut(&mut self) -> Result<&mut crate::inner::FrameSlot> {
        self.ctx
            .inner
            .frames
            .get_mut(&self.id.0)
            .ok_or(Error::UnknownFrame)
    }

    fn slot(&self) -> Result<&crate::inner::FrameSlot> {
        self.ctx
            .inner
            .frames
            .get(&self.id.0)
            .ok_or(Error::UnknownFrame)
    }

    /// Returns the underlying `wl_surface` for the frame's content.
    pub fn wl_surface(&self) -> Result<&WlSurface> {
        Ok(&self.slot()?.wl_surface)
    }

    /// Returns the underlying `xdg_surface`.
    pub fn xdg_surface(&self) -> Result<&XdgSurface> {
        Ok(&self.slot()?.xdg_surface)
    }

    /// Returns the underlying `xdg_toplevel`.
    pub fn xdg_toplevel(&self) -> Result<&XdgToplevel> {
        Ok(&self.slot()?.xdg_toplevel)
    }

    /// Set the window title.
    pub fn set_title(&mut self, title: &str) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.xdg_toplevel.set_title(title.to_owned());
        slot.title = Some(title.to_owned());
        Ok(())
    }

    /// Get the currently set window title.
    pub fn title(&self) -> Result<Option<&str>> {
        Ok(self.slot()?.title.as_deref())
    }

    /// Set the application id (`app_id` in xdg-shell).
    pub fn set_app_id(&mut self, app_id: &str) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.xdg_toplevel.set_app_id(app_id.to_owned());
        slot.app_id = Some(app_id.to_owned());
        Ok(())
    }

    /// Set the minimum content size.
    pub fn set_min_content_size(&mut self, width: i32, height: i32) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.min_size = Some((width, height));
        slot.xdg_toplevel.set_min_size(width, height);
        Ok(())
    }

    /// Set the maximum content size. Pass `0` to leave a dimension
    /// unbounded.
    pub fn set_max_content_size(&mut self, width: i32, height: i32) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.max_size = Some((width, height));
        slot.xdg_toplevel.set_max_size(width, height);
        Ok(())
    }

    /// Get the minimum content size, if one has been set.
    pub fn min_content_size(&self) -> Result<Option<(i32, i32)>> {
        Ok(self.slot()?.min_size)
    }

    /// Get the maximum content size, if one has been set.
    pub fn max_content_size(&self) -> Result<Option<(i32, i32)>> {
        Ok(self.slot()?.max_size)
    }

    /// Add to the frame's capability set.
    pub fn set_capabilities(&mut self, caps: Capabilities) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.capabilities |= caps;
        Ok(())
    }

    /// Remove capabilities from the frame.
    pub fn unset_capabilities(&mut self, caps: Capabilities) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.capabilities = slot.capabilities - caps;
        Ok(())
    }

    /// True if the frame has every capability in `caps`.
    pub fn has_capability(&self, caps: Capabilities) -> Result<bool> {
        Ok(self.slot()?.capabilities.contains(caps))
    }

    /// Get all capabilities.
    pub fn capabilities(&self) -> Result<Capabilities> {
        Ok(self.slot()?.capabilities)
    }

    /// Get the compositor-advertised window manager capabilities.
    pub fn wm_capabilities(&self) -> Result<WmCapabilities> {
        Ok(self.slot()?.wm_capabilities)
    }

    /// Get the current applied window state.
    pub fn window_state(&self) -> Result<WindowState> {
        Ok(self.slot()?.window_state)
    }

    /// Get the current applied content size.
    pub fn content_size(&self) -> Result<(i32, i32)> {
        Ok(self.slot()?.content_size)
    }

    /// True when the frame is in a floating state.
    pub fn is_floating(&self) -> Result<bool> {
        Ok(self.window_state()?.is_floating())
    }

    /// Set or unset the visibility of the frame's decorations.
    ///
    /// libdecor uses this to support borderless windows. The underlying
    /// `wl_surface` is always present; only the (possibly client-drawn)
    /// decoration is toggled.
    pub fn set_visibility(&mut self, visible: bool) -> Result<()> {
        let slot = self.slot_mut()?;
        slot.visible = visible;
        Ok(())
    }

    /// Get the current decoration visibility.
    pub fn is_visible(&self) -> Result<bool> {
        Ok(self.slot()?.visible)
    }

    /// Map the window. The compositor will respond with the initial
    /// configure.
    pub fn map(&mut self) -> Result<()> {
        let slot = self.slot_mut()?;
        if !slot.mapped {
            slot.wl_surface.commit();
            slot.mapped = true;
        }
        Ok(())
    }

    /// Request the compositor to maximize the window.
    pub fn set_maximized(&mut self) -> Result<()> {
        self.slot_mut()?.xdg_toplevel.set_maximized();
        Ok(())
    }

    /// Request the compositor to un-maximize the window.
    pub fn unset_maximized(&mut self) -> Result<()> {
        self.slot_mut()?.xdg_toplevel.unset_maximized();
        Ok(())
    }

    /// Request the compositor to fullscreen the window on the given
    /// output (or let it choose, if `None`).
    pub fn set_fullscreen(
        &mut self,
        output: Option<&wayland_client::protocol::wl_output::WlOutput>,
    ) -> Result<()> {
        self.slot_mut()?.xdg_toplevel.set_fullscreen(output);
        Ok(())
    }

    /// Request the compositor to leave fullscreen.
    pub fn unset_fullscreen(&mut self) -> Result<()> {
        self.slot_mut()?.xdg_toplevel.unset_fullscreen();
        Ok(())
    }

    /// Request the compositor to minimize the window.
    pub fn set_minimized(&mut self) -> Result<()> {
        self.slot_mut()?.xdg_toplevel.set_minimized();
        Ok(())
    }

    /// Close the window. Mirrors `xdg_toplevel::destroy` semantics by
    /// freeing the frame.
    pub fn close(self) -> Result<()> {
        self.ctx.destroy_frame(self.id)
    }

    /// Start an interactive move on the given seat with the given serial.
    pub fn r#move(&mut self, seat: &WlSeat, serial: u32) -> Result<()> {
        self.slot_mut()?.xdg_toplevel._move(seat, serial);
        Ok(())
    }

    /// Start an interactive resize on the given edge.
    pub fn resize(&mut self, seat: &WlSeat, serial: u32, edge: ResizeEdge) -> Result<()> {
        self.slot_mut()?
            .xdg_toplevel
            .resize(seat, serial, edge.to_xdg());
        Ok(())
    }

    /// Show the window menu at a frame-local coordinate.
    pub fn show_window_menu(&mut self, seat: &WlSeat, serial: u32, x: i32, y: i32) -> Result<()> {
        self.slot_mut()?
            .xdg_toplevel
            .show_window_menu(seat, serial, x, y);
        Ok(())
    }

    /// Set the parent frame for stacking.
    pub fn set_parent(&mut self, parent: Option<FrameId>) -> Result<()> {
        let parent_toplevel = if let Some(id) = parent {
            let p = self
                .ctx
                .inner
                .frames
                .get(&id.0)
                .ok_or(Error::UnknownFrame)?;
            Some(p.xdg_toplevel.clone())
        } else {
            None
        };
        let slot = self.slot_mut()?;
        slot.xdg_toplevel.set_parent(parent_toplevel.as_ref());
        Ok(())
    }

    /// Commit a new content state in response to a configure event (or
    /// proactively after an app-driven resize).
    ///
    /// When `configuration` is `Some`, the configure serial is
    /// acknowledged. The content size in `state` becomes the new applied
    /// size. If libdecor is drawing the decorations, the titlebar is
    /// re-rendered at the new width and its `wl_subsurface` committed.
    pub fn commit(&mut self, state: &State, configuration: Option<&Configuration>) -> Result<()> {
        let id = self.id;
        let shm = self.ctx.inner.shm.clone();
        let qh = self.ctx.inner.qh.clone();

        let slot = self.slot_mut()?;
        if let Some(cfg) = configuration {
            slot.xdg_surface.ack_configure(cfg.serial);
            if let Some(ws) = cfg.window_state {
                slot.window_state = ws;
            }
        }
        slot.content_size = (state.content_width, state.content_height);

        let (top, bottom, left, right) = slot.decoration_overhead();
        if top > 0 || bottom > 0 || left > 0 || right > 0 {
            slot.xdg_surface.set_window_geometry(
                -left,
                -top,
                state.content_width + left + right,
                state.content_height + top + bottom,
            );
        }

        if let Some(dec) = slot.csd.as_mut() {
            dec.active = slot.window_state.contains(WindowState::ACTIVE);
            dec.buttons = compute_buttons(
                slot.capabilities,
                slot.wm_capabilities,
                slot.wm_capabilities_known,
            );
            dec.render(&shm, &qh, state.content_width, state.content_height)?;
        }

        let _ = id;
        Ok(())
    }

    /// Translate surface-local coordinates to frame-local coordinates.
    ///
    /// For a frame without client-side decorations these are identical;
    /// with CSD active, the titlebar contributes a vertical offset.
    pub fn translate_coordinate(&self, surface_x: i32, surface_y: i32) -> Result<(i32, i32)> {
        let slot = self.slot()?;
        Ok((surface_x, surface_y + slot.top_decoration()))
    }
}
