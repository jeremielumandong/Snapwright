//! Linux: the Wayland backend in a Wayland session, X11 otherwise.

use crate::{Display, Frame, Result, WindowInfo, wayland, x11};

#[allow(clippy::large_enum_variant)] // one per process, never moved after creation
pub(crate) enum Backend {
    X11(x11::Backend),
    Wayland(wayland::Backend),
}

impl Backend {
    pub(crate) fn new() -> Result<Self> {
        if crate::wayland_session() {
            Ok(Self::Wayland(wayland::Backend::new()?))
        } else {
            Ok(Self::X11(x11::Backend::new()?))
        }
    }

    pub(crate) fn displays(&mut self) -> Result<Vec<Display>> {
        match self {
            Self::X11(b) => b.displays(),
            Self::Wayland(b) => b.displays(),
        }
    }

    pub(crate) fn capture(&mut self, display: &Display) -> Result<Frame> {
        match self {
            Self::X11(b) => b.capture(display),
            Self::Wayland(b) => b.capture(display),
        }
    }

    pub(crate) fn windows(&mut self) -> Result<Vec<WindowInfo>> {
        match self {
            Self::X11(b) => b.windows(),
            Self::Wayland(b) => b.windows(),
        }
    }
}
