# AGENTS.md

Guidelines for AI agents working on libdecor-rs.

## Project Overview

libdecor-rs is a pure-Rust reimplementation of [libdecor], the client-side
decoration library for Wayland. The goal is a small, dependency-light crate
that handles xdg-shell wiring, negotiates server-side decorations when the
compositor supports them, and falls back to a minimal client-drawn frame
otherwise. The library does not pull in GTK, Cairo, or Pango.

[libdecor]: https://gitlab.freedesktop.org/libdecor/libdecor

## Development Workflow

### Commits

- **Commit in small chunks** - one logical change per commit
- **Never commit broken state** - all code must compile and pass tests
- **Format before commit** - run `cargo fmt`
- **Fix clippy issues** - run `cargo clippy` and address all warnings

### Commit Messages

Follow conventional commits with imperative mood, one line, <=72 chars:

```
type: message
```

Types:
- `feat:` - new feature
- `fix:` - bug fix
- `refactor:` - code refactoring
- `test:` - tests
- `docs:` - documentation
- `chore:` - maintenance
- `perf:` - performance
- `style:` - formatting
- `ci:` - CI/CD

Multi-line messages are discouraged. Use them only when truly necessary, and
keep the body minimal.

## Code Quality

### Formatting

```bash
cargo fmt
```

### Linting

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

### Testing

```bash
cargo test --all-features
```

### Adding Dependencies

Use `cargo add <crate>` to add dependencies. Do not edit `Cargo.toml`
dependency entries by hand.

Keep dependencies minimal. Prefer small, focused crates.

## Pre-commit Checklist

Before every commit:

1. [ ] `cargo fmt`
2. [ ] `cargo clippy --all-targets -- -D warnings`
3. [ ] `cargo test`
4. [ ] `cargo build`

## Style

- Default to writing no comments. Add one only when the *why* is non-obvious.
- Document modules and all public items with rustdoc (`//!` and `///`).
- Keep files small and focused. Prefer a few short modules over one large one.
- Prefer `Result<T, Error>` over panicking for fallible operations.
- Use `thiserror` for the public error type.

## Project Structure

```
libdecor-rs/
├── src/
│   ├── lib.rs        # public re-exports + module docs
│   ├── error.rs      # Error type
│   ├── state.rs      # enums + small value types
│   ├── context.rs    # Context (display + globals + dispatch)
│   ├── frame.rs      # Frame (per-window decoration)
│   └── ...
├── examples/
│   └── demo.rs       # minimal demo opening a decorated window
└── tests/
```
