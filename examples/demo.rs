//! Minimal libdecor-rs demo: opens a decorated window and fills it with
//! a flat color.

use libdecor::wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_shm::Format, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use libdecor::{Context, Event, ShmBuffer, State};

struct Pixmap {
    shm: ShmBuffer,
    pool: WlShmPool,
    buffer: WlBuffer,
    width: i32,
    height: i32,
}

impl Pixmap {
    fn new(ctx: &Context, width: i32, height: i32) -> libdecor::Result<Self> {
        let stride = width * 4;
        let len = (stride * height) as usize;
        let shm = ShmBuffer::new(len)?;

        let qh = ctx.queue_handle();
        let pool = ctx.wl_shm().create_pool(shm.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Xrgb8888, qh, ());

        Ok(Self {
            shm,
            pool,
            buffer,
            width,
            height,
        })
    }

    fn fill(&mut self, color: u32) {
        self.shm.as_pixels().fill(color);
    }

    fn attach(&self, surface: &WlSurface) {
        surface.attach(Some(&self.buffer), 0, 0);
        surface.damage_buffer(0, 0, self.width, self.height);
    }
}

impl Drop for Pixmap {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

fn main() -> libdecor::Result<()> {
    let mut ctx = Context::connect()?;
    println!(
        "server-side decorations: {}",
        ctx.supports_server_side_decorations()
    );

    let frame_id = ctx.create_frame()?;
    {
        let mut frame = ctx.frame(frame_id).unwrap();
        frame.set_title("libdecor-rs demo")?;
        frame.set_app_id("io.example.libdecor-demo")?;
        frame.set_min_content_size(200, 150)?;
        frame.map()?;
    }
    ctx.flush()?;

    let mut current: Option<Pixmap> = None;
    let mut running = true;

    while running {
        ctx.dispatch(None)?;
        while let Some(event) = ctx.poll_event() {
            match event {
                Event::Configure {
                    frame,
                    configuration,
                } => {
                    let (w, h) = configuration.content_size().unwrap_or((640, 480));
                    let state = State::new(w, h);

                    let mut pixmap = Pixmap::new(&ctx, w, h)?;
                    pixmap.fill(0xff_1e_1e_2e);
                    let mut f = ctx.frame(frame).unwrap();
                    f.commit(&state, Some(&configuration))?;
                    let surface = f.wl_surface()?;
                    pixmap.attach(surface);
                    surface.commit();
                    current = Some(pixmap);
                }
                Event::Close { .. } => {
                    running = false;
                }
                _ => {}
            }
        }
    }

    drop(current);
    Ok(())
}
