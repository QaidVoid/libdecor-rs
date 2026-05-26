//! Client-side decoration rendering.
//!
//! When the compositor refuses or cannot provide server-side
//! decorations, libdecor draws its own titlebar and four resize
//! borders using SHM-backed subsurfaces. This module owns that state
//! and provides the drawing routines.

use wayland_client::QueueHandle;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_shm::Format, wl_shm::WlShm,
    wl_shm_pool::WlShmPool, wl_subcompositor::WlSubcompositor, wl_subsurface::WlSubsurface,
    wl_surface::WlSurface,
};

use crate::error::Result;
use crate::input::BorderEdge;
use crate::shm::ShmBuffer;

/// Height of the titlebar in surface-local pixels.
pub(crate) const TITLEBAR_HEIGHT: i32 = 32;

/// Thickness of the resize borders.
pub(crate) const BORDER_WIDTH: i32 = 4;

/// Width of the drop shadow extending outside the borders. Drawn into
/// the border subsurfaces with a transparent gradient. Does not
/// contribute to `xdg_surface.set_window_geometry`.
const SHADOW_WIDTH: i32 = 12;

/// Peak alpha of the shadow at the edge nearest the window (0..=255).
const SHADOW_PEAK_ALPHA: u32 = 110;

/// Side length of each titlebar button (close/maximize/minimize).
const BUTTON_SIZE: i32 = 22;

/// Horizontal padding from the right edge of the titlebar.
const BUTTON_RIGHT_PADDING: i32 = 6;

/// Gap between buttons.
const BUTTON_GAP: i32 = 2;

/// Identifies one of the three titlebar buttons.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum ButtonKind {
    /// Close the window.
    Close,
    /// Toggle maximized state.
    Maximize,
    /// Minimize the window.
    Minimize,
}

/// CSD state attached to a frame whose decorations libdecor draws.
pub(crate) struct Decoration {
    pub(crate) titlebar: Subsurface,
    /// Resize borders: indexed by [`BorderEdge`] in the order
    /// `[Top, Bottom, Left, Right]`.
    pub(crate) borders: [Subsurface; 4],
    /// Whether the window currently has keyboard focus.
    pub(crate) active: bool,
    /// Which button (if any) the pointer is currently hovering.
    pub(crate) hover: Option<ButtonKind>,
    /// Which button the user pressed on; releasing on the same button
    /// triggers the action.
    pub(crate) pressed: Option<ButtonKind>,
    /// Which titlebar buttons are visible. Updated each render based on
    /// the client's [`Capabilities`](crate::Capabilities) and the
    /// compositor's [`WmCapabilities`](crate::WmCapabilities).
    pub(crate) buttons: ButtonSet,
}

/// Bitfield-ish struct describing which titlebar buttons are visible.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct ButtonSet {
    pub(crate) close: bool,
    pub(crate) maximize: bool,
    pub(crate) minimize: bool,
}

impl ButtonSet {
    pub(crate) const fn all() -> Self {
        Self {
            close: true,
            maximize: true,
            minimize: true,
        }
    }

    fn visible_right_to_left(self) -> impl Iterator<Item = ButtonKind> {
        [
            (self.close, ButtonKind::Close),
            (self.maximize, ButtonKind::Maximize),
            (self.minimize, ButtonKind::Minimize),
        ]
        .into_iter()
        .filter_map(|(show, k)| show.then_some(k))
    }
}

/// A single decoration subsurface.
pub(crate) struct Subsurface {
    pub(crate) wl_surface: WlSurface,
    pub(crate) wl_subsurface: WlSubsurface,
    pub(crate) current: Option<RenderedBuffer>,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// A live wl_shm-backed `wl_buffer` together with its mmap and pool.
pub(crate) struct RenderedBuffer {
    pub(crate) shm: ShmBuffer,
    pub(crate) pool: WlShmPool,
    pub(crate) buffer: WlBuffer,
}

impl Decoration {
    pub(crate) fn new(
        compositor: &WlCompositor,
        subcompositor: &WlSubcompositor,
        parent: &WlSurface,
        qh: &QueueHandle<crate::inner::Inner>,
    ) -> Self {
        let titlebar = make_subsurface(compositor, subcompositor, parent, qh);
        let borders = [
            make_subsurface(compositor, subcompositor, parent, qh),
            make_subsurface(compositor, subcompositor, parent, qh),
            make_subsurface(compositor, subcompositor, parent, qh),
            make_subsurface(compositor, subcompositor, parent, qh),
        ];

        Self {
            titlebar,
            borders,
            active: false,
            hover: None,
            pressed: None,
            buttons: ButtonSet::all(),
        }
    }

    pub(crate) fn destroy(self) {
        for sub in std::iter::once(self.titlebar).chain(self.borders) {
            if let Some(rb) = sub.current {
                rb.buffer.destroy();
                rb.pool.destroy();
            }
            sub.wl_subsurface.destroy();
            sub.wl_surface.destroy();
        }
    }

    /// Surface id for the titlebar (used for pointer-event routing).
    pub(crate) fn titlebar_surface_id(&self) -> ObjectId {
        use wayland_client::Proxy;
        self.titlebar.wl_surface.id()
    }

    /// Surface id for the border at the given edge.
    pub(crate) fn border_surface_id(&self, edge: BorderEdge) -> ObjectId {
        use wayland_client::Proxy;
        self.borders[edge_index(edge)].wl_surface.id()
    }

    /// Re-layout all subsurfaces for the given content area and redraw
    /// each. Should be called from `Frame::commit` once the content
    /// width and height are known.
    pub(crate) fn render(
        &mut self,
        compositor: &WlCompositor,
        shm: &WlShm,
        qh: &QueueHandle<crate::inner::Inner>,
        content_width: i32,
        content_height: i32,
        title: Option<&str>,
    ) -> Result<()> {
        if content_width <= 0 || content_height <= 0 {
            return Ok(());
        }

        // Titlebar
        self.titlebar
            .wl_subsurface
            .set_position(0, -TITLEBAR_HEIGHT);
        let (active, hover, pressed, buttons) =
            (self.active, self.hover, self.pressed, self.buttons);
        render_subsurface(
            &mut self.titlebar,
            shm,
            qh,
            content_width,
            TITLEBAR_HEIGHT,
            Format::Argb8888,
            |pixels, w, h| draw_titlebar(pixels, w, h, active, hover, pressed, buttons, title),
        )?;

        let border_color: u32 = if self.active {
            0xff_45_47_5a
        } else {
            0xff_31_31_44
        };

        let outer = BORDER_WIDTH + SHADOW_WIDTH;
        let h_span = content_width + 2 * outer;
        let v_span = TITLEBAR_HEIGHT + content_height;

        // Top: shadow above, solid border under titlebar.
        self.borders[edge_index(BorderEdge::Top)]
            .wl_subsurface
            .set_position(-outer, -TITLEBAR_HEIGHT - BORDER_WIDTH - SHADOW_WIDTH);
        render_border(
            &mut self.borders[edge_index(BorderEdge::Top)],
            compositor,
            shm,
            qh,
            h_span,
            outer,
            BorderEdge::Top,
            border_color,
        )?;

        // Bottom: solid border first, shadow below.
        self.borders[edge_index(BorderEdge::Bottom)]
            .wl_subsurface
            .set_position(-outer, content_height);
        render_border(
            &mut self.borders[edge_index(BorderEdge::Bottom)],
            compositor,
            shm,
            qh,
            h_span,
            outer,
            BorderEdge::Bottom,
            border_color,
        )?;

        // Left: shadow on the outer side, solid strip adjacent to the
        // content. Does not include corner regions (those are covered by
        // the top and bottom subsurfaces).
        self.borders[edge_index(BorderEdge::Left)]
            .wl_subsurface
            .set_position(-BORDER_WIDTH - SHADOW_WIDTH, -TITLEBAR_HEIGHT);
        render_border(
            &mut self.borders[edge_index(BorderEdge::Left)],
            compositor,
            shm,
            qh,
            outer,
            v_span,
            BorderEdge::Left,
            border_color,
        )?;

        // Right: same as left but mirrored.
        self.borders[edge_index(BorderEdge::Right)]
            .wl_subsurface
            .set_position(content_width, -TITLEBAR_HEIGHT);
        render_border(
            &mut self.borders[edge_index(BorderEdge::Right)],
            compositor,
            shm,
            qh,
            outer,
            v_span,
            BorderEdge::Right,
            border_color,
        )?;

        Ok(())
    }

    /// Hit-test a titlebar-local pointer position against the buttons.
    pub(crate) fn hit_test(&self, x: f64, y: f64) -> Option<ButtonKind> {
        if y < 0.0 || y > TITLEBAR_HEIGHT as f64 {
            return None;
        }
        for (idx, kind) in self.buttons.visible_right_to_left().enumerate() {
            let bx = button_x(self.titlebar.width, idx as i32);
            if x >= bx as f64 && x < (bx + BUTTON_SIZE) as f64 {
                let by = (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2;
                if y >= by as f64 && y < (by + BUTTON_SIZE) as f64 {
                    return Some(kind);
                }
            }
        }
        None
    }
}

pub(crate) fn edge_index(edge: BorderEdge) -> usize {
    match edge {
        BorderEdge::Top => 0,
        BorderEdge::Bottom => 1,
        BorderEdge::Left => 2,
        BorderEdge::Right => 3,
    }
}

fn make_subsurface(
    compositor: &WlCompositor,
    subcompositor: &WlSubcompositor,
    parent: &WlSurface,
    qh: &QueueHandle<crate::inner::Inner>,
) -> Subsurface {
    let wl_surface = compositor.create_surface(qh, ());
    let wl_subsurface = subcompositor.get_subsurface(&wl_surface, parent, qh, ());
    wl_subsurface.set_sync();
    Subsurface {
        wl_surface,
        wl_subsurface,
        current: None,
        width: 0,
        height: 0,
    }
}

/// Stage `paint` into the subsurface's buffer and attach + damage, but
/// do not commit. The caller is responsible for any further surface
/// state (such as `set_input_region`) and the final `commit`.
fn paint_subsurface<F>(
    sub: &mut Subsurface,
    shm: &WlShm,
    qh: &QueueHandle<crate::inner::Inner>,
    width: i32,
    height: i32,
    format: Format,
    paint: F,
) -> Result<()>
where
    F: FnOnce(&mut [u32], i32, i32),
{
    if width <= 0 || height <= 0 {
        return Ok(());
    }

    let needs_realloc = sub
        .current
        .as_ref()
        .map(|_| sub.width != width || sub.height != height)
        .unwrap_or(true);

    if needs_realloc {
        if let Some(rb) = sub.current.take() {
            rb.buffer.destroy();
            rb.pool.destroy();
        }
        let stride = width * 4;
        let len = (stride * height) as usize;
        let buffer_shm = ShmBuffer::new(len)?;
        let pool = shm.create_pool(buffer_shm.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, format, qh, ());
        sub.current = Some(RenderedBuffer {
            shm: buffer_shm,
            pool,
            buffer,
        });
        sub.width = width;
        sub.height = height;
    }

    let rb = sub.current.as_mut().unwrap();
    paint(rb.shm.as_pixels(), width, height);
    sub.wl_surface.attach(Some(&rb.buffer), 0, 0);
    sub.wl_surface.damage_buffer(0, 0, width, height);
    Ok(())
}

fn render_subsurface<F>(
    sub: &mut Subsurface,
    shm: &WlShm,
    qh: &QueueHandle<crate::inner::Inner>,
    width: i32,
    height: i32,
    format: Format,
    paint: F,
) -> Result<()>
where
    F: FnOnce(&mut [u32], i32, i32),
{
    paint_subsurface(sub, shm, qh, width, height, format, paint)?;
    if width > 0 && height > 0 {
        sub.wl_surface.commit();
    }
    Ok(())
}

/// Render a border subsurface that includes a shadow gradient on its
/// outer side. The subsurface's input region is restricted to the
/// solid border strip so the shadow does not capture pointer events.
#[allow(clippy::too_many_arguments)]
fn render_border(
    sub: &mut Subsurface,
    compositor: &WlCompositor,
    shm: &WlShm,
    qh: &QueueHandle<crate::inner::Inner>,
    width: i32,
    height: i32,
    edge: BorderEdge,
    border_color: u32,
) -> Result<()> {
    paint_subsurface(
        sub,
        shm,
        qh,
        width,
        height,
        Format::Argb8888,
        |pixels, w, h| draw_border(pixels, w, h, edge, border_color),
    )?;
    if width <= 0 || height <= 0 {
        return Ok(());
    }

    let region = compositor.create_region(qh, ());
    let (rx, ry, rw, rh) = border_input_rect(edge, width, height);
    region.add(rx, ry, rw, rh);
    sub.wl_surface.set_input_region(Some(&region));
    sub.wl_surface.commit();
    region.destroy();
    Ok(())
}

/// Returns the rectangular input region (within the subsurface's local
/// coordinates) that the border accepts pointer events on. The shadow
/// extension lies outside this rect and is click-through.
fn border_input_rect(edge: BorderEdge, w: i32, h: i32) -> (i32, i32, i32, i32) {
    match edge {
        BorderEdge::Top => {
            // Width-spanning top subsurface: shadow on top, solid strip
            // in the bottom BORDER_WIDTH rows of the central column.
            (
                BORDER_WIDTH + SHADOW_WIDTH,
                SHADOW_WIDTH,
                w - 2 * (BORDER_WIDTH + SHADOW_WIDTH),
                BORDER_WIDTH,
            )
        }
        BorderEdge::Bottom => (
            BORDER_WIDTH + SHADOW_WIDTH,
            0,
            w - 2 * (BORDER_WIDTH + SHADOW_WIDTH),
            BORDER_WIDTH,
        ),
        BorderEdge::Left => (SHADOW_WIDTH, 0, BORDER_WIDTH, h),
        BorderEdge::Right => (0, 0, BORDER_WIDTH, h),
    }
}

/// Paint a border subsurface: shadow gradient on the outer side, solid
/// color on the inner side.
fn draw_border(pixels: &mut [u32], width: i32, height: i32, edge: BorderEdge, color: u32) {
    pixels.fill(0);

    match edge {
        BorderEdge::Top => {
            // Solid strip occupies the bottom BORDER_WIDTH rows of the
            // central column (excluding the side-shadow extensions).
            let x0 = BORDER_WIDTH + SHADOW_WIDTH;
            let x1 = width - x0;
            let y_solid = SHADOW_WIDTH;
            for y in y_solid..(y_solid + BORDER_WIDTH) {
                fill_row(pixels, width, y, x0, x1, color);
            }
            // Vertical shadow above the solid strip and its corners.
            for y in 0..SHADOW_WIDTH {
                let dist = SHADOW_WIDTH - y;
                let alpha = shadow_alpha(dist);
                let px = premultiplied_black(alpha);
                fill_row(pixels, width, y, x0, x1, px);
            }
            // Corner shadows (above the side-extensions).
            draw_corner_shadow(pixels, width, height, edge);
        }
        BorderEdge::Bottom => {
            let x0 = BORDER_WIDTH + SHADOW_WIDTH;
            let x1 = width - x0;
            for y in 0..BORDER_WIDTH {
                fill_row(pixels, width, y, x0, x1, color);
            }
            for y in BORDER_WIDTH..(BORDER_WIDTH + SHADOW_WIDTH) {
                let dist = y - BORDER_WIDTH + 1;
                let alpha = shadow_alpha(dist);
                let px = premultiplied_black(alpha);
                fill_row(pixels, width, y, x0, x1, px);
            }
            draw_corner_shadow(pixels, width, height, edge);
        }
        BorderEdge::Left => {
            // Solid strip: right-most BORDER_WIDTH columns.
            let x_solid_start = SHADOW_WIDTH;
            for y in 0..height {
                fill_row(
                    pixels,
                    width,
                    y,
                    x_solid_start,
                    x_solid_start + BORDER_WIDTH,
                    color,
                );
            }
            for x in 0..SHADOW_WIDTH {
                let dist = SHADOW_WIDTH - x;
                let alpha = shadow_alpha(dist);
                let px = premultiplied_black(alpha);
                for y in 0..height {
                    set_pixel(pixels, width, x, y, px);
                }
            }
        }
        BorderEdge::Right => {
            for y in 0..height {
                fill_row(pixels, width, y, 0, BORDER_WIDTH, color);
            }
            for x in BORDER_WIDTH..(BORDER_WIDTH + SHADOW_WIDTH) {
                let dist = x - BORDER_WIDTH + 1;
                let alpha = shadow_alpha(dist);
                let px = premultiplied_black(alpha);
                for y in 0..height {
                    set_pixel(pixels, width, x, y, px);
                }
            }
        }
    }
}

/// Draw soft 2-D shadow in the corner regions of the top / bottom
/// border subsurfaces (outside the central solid strip). Distance to
/// the nearest visible-window corner determines the alpha.
fn draw_corner_shadow(pixels: &mut [u32], width: i32, height: i32, edge: BorderEdge) {
    let inner_x_left = BORDER_WIDTH + SHADOW_WIDTH;
    let inner_x_right = width - inner_x_left;
    let (inner_y_top, inner_y_bot) = match edge {
        BorderEdge::Top => (SHADOW_WIDTH, height),
        BorderEdge::Bottom => (0, BORDER_WIDTH),
        _ => return,
    };
    for y in 0..height {
        for x in 0..width {
            if x >= inner_x_left && x < inner_x_right {
                continue;
            }
            // Nearest point on the visible window: clamp (x, y) to the
            // solid rectangle [inner_x_left, inner_x_right) x
            // [inner_y_top, inner_y_bot).
            let nx = x.clamp(inner_x_left, inner_x_right - 1);
            let ny = y.clamp(inner_y_top, inner_y_bot - 1);
            let dx = nx - x;
            let dy = ny - y;
            let dist = ((dx * dx + dy * dy) as f32).sqrt() as i32;
            if dist > SHADOW_WIDTH {
                continue;
            }
            let alpha = shadow_alpha((SHADOW_WIDTH - dist).max(0));
            set_pixel(pixels, width, x, y, premultiplied_black(alpha));
        }
    }
}

fn shadow_alpha(distance_from_outer: i32) -> u32 {
    let t = distance_from_outer.max(0) as f32 / SHADOW_WIDTH as f32;
    let t = t.min(1.0);
    (t * SHADOW_PEAK_ALPHA as f32) as u32
}

fn premultiplied_black(alpha: u32) -> u32 {
    (alpha & 0xff) << 24
}

fn fill_row(pixels: &mut [u32], stride: i32, y: i32, x0: i32, x1: i32, color: u32) {
    if y < 0 || x1 <= x0 {
        return;
    }
    let stride_u = stride as usize;
    let max_y = pixels.len() / stride_u.max(1);
    if (y as usize) >= max_y {
        return;
    }
    let xs = x0.max(0) as usize;
    let xe = x1.min(stride) as usize;
    let row_start = (y as usize) * stride_u;
    pixels[row_start + xs..row_start + xe].fill(color);
}

fn set_pixel(pixels: &mut [u32], stride: i32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= stride {
        return;
    }
    let idx = (y as usize) * stride as usize + x as usize;
    if idx < pixels.len() {
        pixels[idx] = color;
    }
}

/// Compute the left edge of the `idx`-th button. Buttons are laid out
/// right-to-left as [Minimize, Maximize, Close].
fn button_x(width: i32, idx: i32) -> i32 {
    let right_edge = width - BUTTON_RIGHT_PADDING;
    right_edge - (idx + 1) * BUTTON_SIZE - idx * BUTTON_GAP
}

/// Paint the titlebar background, title text, and visible buttons.
#[allow(clippy::too_many_arguments)]
fn draw_titlebar(
    pixels: &mut [u32],
    width: i32,
    height: i32,
    active: bool,
    hover: Option<ButtonKind>,
    pressed: Option<ButtonKind>,
    buttons: ButtonSet,
    title: Option<&str>,
) {
    let (bar_color, fg, hover_color, pressed_color, close_pressed) = if active {
        (
            0xff_1e_1e_2e,
            0xff_cd_d6_f4,
            0xff_31_32_44,
            0xff_45_47_5a,
            0xff_e0_6c_75,
        )
    } else {
        (
            0xff_18_18_25,
            0xff_6c_70_86,
            0xff_24_24_36,
            0xff_31_31_44,
            0xff_8b_3c_3c,
        )
    };

    fill_rect(pixels, width, 0, 0, width, height, bar_color);

    if let Some(title) = title.filter(|s| !s.is_empty()) {
        let font_size = 14.0;
        let measured = crate::font::measure(title, font_size);
        let visible_count = buttons.visible_right_to_left().count() as i32;
        let buttons_left = if visible_count > 0 {
            button_x(width, visible_count - 1)
        } else {
            width
        };
        let available = (buttons_left - 12).max(0);
        if measured > 0 && available > 0 {
            let x = ((width - measured) / 2).clamp(12, available.saturating_sub(measured).max(12));
            let y_baseline = (height + (font_size * 0.7) as i32) / 2;
            crate::font::draw_text(pixels, width, height, x, y_baseline, font_size, title, fg);
        }
    }

    for (idx, kind) in buttons.visible_right_to_left().enumerate() {
        let bx = button_x(width, idx as i32);
        let by = (height - BUTTON_SIZE) / 2;
        let bg = if pressed == Some(kind) {
            if kind == ButtonKind::Close {
                close_pressed
            } else {
                pressed_color
            }
        } else if hover == Some(kind) {
            hover_color
        } else {
            bar_color
        };
        fill_rect(pixels, width, bx, by, BUTTON_SIZE, BUTTON_SIZE, bg);
        draw_icon(pixels, width, bx, by, BUTTON_SIZE, kind, fg);
    }
}

fn fill_rect(pixels: &mut [u32], stride: i32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let stride = stride as usize;
    let total = pixels.len();
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w).min(stride as i32) as usize;
    let y1 = ((y + h) as usize).min(total / stride.max(1));
    for row in y0..y1 {
        let start = row * stride + x0;
        let end = row * stride + x1;
        pixels[start..end].fill(color);
    }
}

fn put_pixel(pixels: &mut [u32], stride: i32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= stride {
        return;
    }
    let stride = stride as usize;
    let idx = (y as usize) * stride + (x as usize);
    if idx < pixels.len() {
        pixels[idx] = color;
    }
}

fn draw_line(
    pixels: &mut [u32],
    stride: i32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        put_pixel(pixels, stride, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_rect_outline(pixels: &mut [u32], stride: i32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    draw_line(pixels, stride, x, y, x + w - 1, y, color);
    draw_line(pixels, stride, x, y + h - 1, x + w - 1, y + h - 1, color);
    draw_line(pixels, stride, x, y, x, y + h - 1, color);
    draw_line(pixels, stride, x + w - 1, y, x + w - 1, y + h - 1, color);
}

fn draw_icon(
    pixels: &mut [u32],
    stride: i32,
    bx: i32,
    by: i32,
    size: i32,
    kind: ButtonKind,
    color: u32,
) {
    let inset = size / 4;
    let x0 = bx + inset;
    let y0 = by + inset;
    let x1 = bx + size - 1 - inset;
    let y1 = by + size - 1 - inset;

    match kind {
        ButtonKind::Close => {
            draw_line(pixels, stride, x0, y0, x1, y1, color);
            draw_line(pixels, stride, x0 + 1, y0, x1 + 1, y1, color);
            draw_line(pixels, stride, x0, y1, x1, y0, color);
            draw_line(pixels, stride, x0 + 1, y1, x1 + 1, y0, color);
        }
        ButtonKind::Maximize => {
            draw_rect_outline(pixels, stride, x0, y0, x1 - x0, y1 - y0, color);
            draw_line(pixels, stride, x0 + 1, y0 + 1, x1 - 1, y0 + 1, color);
        }
        ButtonKind::Minimize => {
            draw_line(pixels, stride, x0, y1, x1, y1, color);
            draw_line(pixels, stride, x0, y1 - 1, x1, y1 - 1, color);
        }
    }
}
