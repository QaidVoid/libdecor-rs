//! Opaque identifiers for libdecor resources.

/// Identifier for a frame managed by a [`Context`](crate::Context).
///
/// `FrameId`s are returned by
/// [`Context::create_frame`](crate::Context::create_frame) and used to
/// refer back to the same frame across calls. They are `Copy` and have
/// no resource ownership semantics: dropping a `FrameId` does not free
/// the underlying window.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct FrameId(pub(crate) usize);
