# libdecor-rs

A pure-Rust reimplementation of [libdecor], the client-side decoration helper
for Wayland clients.

The goal is a small, dependency-light crate that:

- wires up `xdg_wm_base` / `xdg_surface` / `xdg_toplevel` for you,
- asks the compositor for server-side decorations via
  `xdg-decoration-unstable-v1` when available,
- falls back to a minimal client-drawn titlebar otherwise,
- never pulls in GTK, Cairo, or Pango.

[libdecor]: https://gitlab.freedesktop.org/libdecor/libdecor

## Status

Work in progress. The public API mirrors libdecor's C interface conceptually
but uses idiomatic Rust types.

## Quick Look

```rust,no_run
use libdecor::{Context, FrameHandler, WindowState};

struct App;

impl FrameHandler for App {
    fn configure(&mut self, frame: &mut libdecor::Frame, cfg: libdecor::Configuration) {
        let (w, h) = cfg.content_size(frame).unwrap_or((640, 480));
        let state = libdecor::State::new(w, h);
        frame.commit(&state, Some(&cfg));
    }

    fn close(&mut self, _frame: &mut libdecor::Frame) {
        std::process::exit(0);
    }
}

fn main() {
    let mut ctx = Context::connect().unwrap();
    let _frame = ctx.decorate("demo", App).unwrap();
    loop {
        ctx.dispatch(None).unwrap();
    }
}
```

## License

MIT
