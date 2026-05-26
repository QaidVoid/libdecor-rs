//! Small value types that describe window state, configuration, and
//! capabilities.
//!
//! These mirror the corresponding C enums in libdecor but use idiomatic
//! Rust bitflags-style structs without pulling in a `bitflags` dependency.

use wayland_protocols::xdg::shell::client::xdg_toplevel;

macro_rules! flags {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident: $repr:ty {
            $(
                $(#[$fmeta:meta])*
                const $flag:ident = $value:expr;
            )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Eq, PartialEq, Default)]
        $vis struct $name($repr);

        impl $name {
            /// Empty flag set.
            pub const NONE: Self = Self(0);

            $(
                $(#[$fmeta])*
                pub const $flag: Self = Self($value);
            )+

            /// Returns the raw bitmask.
            #[inline]
            pub const fn bits(self) -> $repr {
                self.0
            }

            /// Constructs from a raw bitmask, masked to known bits.
            #[inline]
            pub const fn from_bits_truncate(bits: $repr) -> Self {
                Self(bits & Self::all().0)
            }

            /// Returns the union of all defined flags.
            pub const fn all() -> Self {
                Self($( $value | )+ 0)
            }

            /// True when `self` contains every flag in `other`.
            #[inline]
            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }

            /// True when `self` and `other` share any flag.
            #[inline]
            pub const fn intersects(self, other: Self) -> bool {
                (self.0 & other.0) != 0
            }

            /// Returns `self` with all flags from `other` added.
            #[inline]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            /// Returns `self` with all flags from `other` removed.
            #[inline]
            pub const fn difference(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }

            /// Returns `true` if no flags are set.
            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl std::ops::BitOr for $name {
            type Output = Self;
            #[inline]
            fn bitor(self, rhs: Self) -> Self { self.union(rhs) }
        }

        impl std::ops::BitOrAssign for $name {
            #[inline]
            fn bitor_assign(&mut self, rhs: Self) { *self = self.union(rhs); }
        }

        impl std::ops::BitAnd for $name {
            type Output = Self;
            #[inline]
            fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
        }

        impl std::ops::Sub for $name {
            type Output = Self;
            #[inline]
            fn sub(self, rhs: Self) -> Self { self.difference(rhs) }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut first = true;
                f.write_str(stringify!($name))?;
                f.write_str("(")?;
                $(
                    if self.contains(Self::$flag) {
                        if !first { f.write_str(" | ")?; }
                        f.write_str(stringify!($flag))?;
                        first = false;
                    }
                )+
                if first { f.write_str("NONE")?; }
                f.write_str(")")
            }
        }
    };
}

flags! {
    /// State flags reported by the compositor for a toplevel window.
    pub struct WindowState: u32 {
        /// The window currently has keyboard focus.
        const ACTIVE             = 1 << 0;
        /// The window is maximized.
        const MAXIMIZED          = 1 << 1;
        /// The window is fullscreen.
        const FULLSCREEN         = 1 << 2;
        /// The window is tiled along its left edge.
        const TILED_LEFT         = 1 << 3;
        /// The window is tiled along its right edge.
        const TILED_RIGHT        = 1 << 4;
        /// The window is tiled along its top edge.
        const TILED_TOP          = 1 << 5;
        /// The window is tiled along its bottom edge.
        const TILED_BOTTOM       = 1 << 6;
        /// The window is suspended (offscreen / not visible).
        const SUSPENDED          = 1 << 7;
        /// The window is in the middle of an interactive resize.
        const RESIZING           = 1 << 8;
        /// The window is constrained from moving left.
        const CONSTRAINED_LEFT   = 1 << 9;
        /// The window is constrained from moving right.
        const CONSTRAINED_RIGHT  = 1 << 10;
        /// The window is constrained from moving up.
        const CONSTRAINED_TOP    = 1 << 11;
        /// The window is constrained from moving down.
        const CONSTRAINED_BOTTOM = 1 << 12;
    }
}

impl WindowState {
    const NON_FLOATING: Self = Self(
        Self::MAXIMIZED.0
            | Self::FULLSCREEN.0
            | Self::TILED_LEFT.0
            | Self::TILED_RIGHT.0
            | Self::TILED_TOP.0
            | Self::TILED_BOTTOM.0,
    );

    /// Returns `true` when the window is not in any tiled, maximized, or
    /// fullscreen state.
    pub const fn is_floating(self) -> bool {
        !self.intersects(Self::NON_FLOATING)
    }
}

flags! {
    /// Capabilities the client advertises to the decoration layer.
    ///
    /// Setting a capability makes the corresponding action available in
    /// the window menu and titlebar (for example, showing a close button
    /// when `CLOSE` is set).
    pub struct Capabilities: u32 {
        /// The window can be moved.
        const MOVE       = 1 << 0;
        /// The window can be resized.
        const RESIZE     = 1 << 1;
        /// The window can be minimized.
        const MINIMIZE   = 1 << 2;
        /// The window can be fullscreened.
        const FULLSCREEN = 1 << 3;
        /// The window can be closed.
        const CLOSE      = 1 << 4;
    }
}

impl Capabilities {
    /// All actions enabled.
    pub const fn full() -> Self {
        Self::all()
    }
}

flags! {
    /// Capabilities advertised by the window manager / compositor.
    pub struct WmCapabilities: u32 {
        /// The compositor supports the standard window menu.
        const WINDOW_MENU = 1 << 0;
        /// The compositor supports maximizing windows.
        const MAXIMIZE    = 1 << 1;
        /// The compositor supports fullscreen windows.
        const FULLSCREEN  = 1 << 2;
        /// The compositor supports minimizing windows.
        const MINIMIZE    = 1 << 3;
    }
}

/// Which edge or corner of a window an interactive resize is acting on.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum ResizeEdge {
    /// No edge selected.
    #[default]
    None,
    /// Top edge.
    Top,
    /// Bottom edge.
    Bottom,
    /// Left edge.
    Left,
    /// Top-left corner.
    TopLeft,
    /// Bottom-left corner.
    BottomLeft,
    /// Right edge.
    Right,
    /// Top-right corner.
    TopRight,
    /// Bottom-right corner.
    BottomRight,
}

impl ResizeEdge {
    #[allow(dead_code)]
    pub(crate) fn to_xdg(self) -> xdg_toplevel::ResizeEdge {
        use xdg_toplevel::ResizeEdge as X;
        match self {
            Self::None => X::None,
            Self::Top => X::Top,
            Self::Bottom => X::Bottom,
            Self::Left => X::Left,
            Self::TopLeft => X::TopLeft,
            Self::BottomLeft => X::BottomLeft,
            Self::Right => X::Right,
            Self::TopRight => X::TopRight,
            Self::BottomRight => X::BottomRight,
        }
    }
}

/// A configured content state, ready to be committed to a frame.
///
/// Created via [`State::new`] and passed to
/// [`Frame::commit`](crate::Frame::commit).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub(crate) content_width: i32,
    pub(crate) content_height: i32,
    pub(crate) window_state: WindowState,
}

impl State {
    /// Create a new state describing the desired content size.
    pub const fn new(width: i32, height: i32) -> Self {
        Self {
            content_width: width,
            content_height: height,
            window_state: WindowState::NONE,
        }
    }

    /// Returns the requested content width.
    pub const fn content_width(&self) -> i32 {
        self.content_width
    }

    /// Returns the requested content height.
    pub const fn content_height(&self) -> i32 {
        self.content_height
    }
}
