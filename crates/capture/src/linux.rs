//! Linux backend. X11 through MIT-SHM, which copies the root window into shared memory
//! in one server-side blit, with a plain `GetImage` when SHM is missing (remote displays).
//!
//! Wayland sessions are refused for now: an X11 `GetImage` under XWayland returns only X
//! clients. The portal and `ext-image-copy-capture` backends land in Phase 2.

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use crate::{CaptureError, Display, Frame, Rect, Result};

fn err(api: &'static str, e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Platform {
        api,
        message: e.to_string(),
    }
}

// SAFETY: the X connection is owned and the shared memory segment is private to this
// backend; `Capturer` takes `&mut self` for every call, so it is never used from two threads
// at once.
unsafe impl Send for Backend {}

pub(crate) struct Backend {
    conn: RustConnection,
    root: Window,
    screen: Rect,
    shm: Option<ShmSegment>,
    shm_available: bool,
}

/// A SysV shared memory segment attached to both this process and the X server.
struct ShmSegment {
    seg: shm::Seg,
    addr: *mut u8,
    size: usize,
}

impl Backend {
    pub(crate) fn new() -> Result<Self> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
            || (std::env::var_os("DISPLAY").is_none()
                && std::env::var_os("WAYLAND_DISPLAY").is_some())
        {
            return Err(CaptureError::Unsupported(
                "Wayland sessions need the portal backend (planned for Phase 2)".into(),
            ));
        }
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| err("XOpenDisplay", e))?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let bpp = conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == screen.root_depth)
            .map(|f| f.bits_per_pixel);
        if bpp != Some(32) {
            return Err(CaptureError::Unsupported(format!(
                "root window depth {} with {bpp:?} bits per pixel",
                screen.root_depth
            )));
        }
        let screen_rect = Rect::new(
            0,
            0,
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        );
        let shm_available = conn
            .extension_information(shm::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some()
            && !is_remote_display();
        Ok(Self {
            conn,
            root,
            screen: screen_rect,
            shm: None,
            shm_available,
        })
    }

    pub(crate) fn displays(&mut self) -> Result<Vec<Display>> {
        let monitors = self
            .conn
            .randr_get_monitors(self.root, true)
            .map_err(|e| err("RRGetMonitors", e))?
            .reply();
        let monitors = match monitors {
            Ok(m) if !m.monitors.is_empty() => m.monitors,
            // No RandR 1.5 (Xvfb without it, old servers): the root window is one display.
            _ => {
                return Ok(vec![Display {
                    id: 0,
                    name: "screen".into(),
                    bounds: self.screen,
                    scale: 1.0,
                    primary: true,
                }]);
            }
        };
        let mut out = Vec::with_capacity(monitors.len());
        for m in monitors {
            let name = self
                .conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_else(|| format!("monitor-{}", m.name));
            out.push(Display {
                id: u64::from(m.name),
                name,
                bounds: Rect::new(
                    i32::from(m.x),
                    i32::from(m.y),
                    u32::from(m.width),
                    u32::from(m.height),
                ),
                // X11 has no per-monitor scale; toolkits read Xft.dpi. 1.0 until the app needs it.
                scale: 1.0,
                primary: m.primary,
            });
        }
        Ok(out)
    }

    pub(crate) fn capture(&mut self, display: &Display) -> Result<Frame> {
        let r = display
            .bounds
            .intersect(&self.screen)
            .ok_or(CaptureError::NoSuchDisplay(display.id))?;
        if self.shm_available {
            match self.capture_shm(display.id, r) {
                Ok(frame) => return Ok(frame),
                Err(e) => {
                    tracing::warn!(error = %e, "MIT-SHM capture failed; using GetImage from now on");
                    self.shm_available = false;
                    self.shm = None;
                }
            }
        }
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                r.x as i16,
                r.y as i16,
                r.width as u16,
                r.height as u16,
                !0,
            )
            .map_err(|e| err("GetImage", e))?
            .reply()
            .map_err(|e| err("GetImage", e))?;
        Ok(Frame::from_bgrx_rows(
            display.id,
            r.width,
            r.height,
            r.width as usize * 4,
            &reply.data,
        ))
    }

    fn capture_shm(&mut self, display_id: u64, r: Rect) -> Result<Frame> {
        let size = r.width as usize * r.height as usize * 4;
        if self.shm.as_ref().is_none_or(|s| s.size < size) {
            self.shm = None;
            // Size the segment for the whole root window so every monitor reuses it.
            let full = self.screen.width as usize * self.screen.height as usize * 4;
            self.shm = Some(ShmSegment::new(&self.conn, full.max(size))?);
        }
        let Some(seg) = self.shm.as_ref() else {
            return Err(err("shmget", "no segment"));
        };
        self.conn
            .shm_get_image(
                self.root,
                r.x as i16,
                r.y as i16,
                r.width as u16,
                r.height as u16,
                !0,
                ImageFormat::Z_PIXMAP.into(),
                seg.seg,
                0,
            )
            .map_err(|e| err("ShmGetImage", e))?
            .reply()
            .map_err(|e| err("ShmGetImage", e))?;
        // SAFETY: the segment is `seg.size >= size` bytes, mapped for this process's lifetime of
        // `seg`, and the server finished writing it before the reply above arrived.
        let pixels = unsafe { std::slice::from_raw_parts(seg.addr, size) };
        Ok(Frame::from_bgrx_rows(
            display_id,
            r.width,
            r.height,
            r.width as usize * 4,
            pixels,
        ))
    }
}

impl ShmSegment {
    fn new(conn: &RustConnection, size: usize) -> Result<Self> {
        // SAFETY: plain SysV calls; every return value is checked before use.
        let id = unsafe { libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600) };
        if id < 0 {
            return Err(err("shmget", std::io::Error::last_os_error()));
        }
        // SAFETY: `id` is the segment just created; a null address lets the kernel choose.
        let addr = unsafe { libc::shmat(id, std::ptr::null(), 0) };
        if addr as isize == -1 {
            let e = std::io::Error::last_os_error();
            // SAFETY: removes the segment we created and failed to map.
            unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
            return Err(err("shmat", e));
        }
        let attach = || -> Result<shm::Seg> {
            let seg = conn.generate_id().map_err(|e| err("shm id", e))?;
            conn.shm_attach(seg, id as u32, false)
                .map_err(|e| err("ShmAttach", e))?
                .check()
                .map_err(|e| err("ShmAttach", e))?;
            Ok(seg)
        };
        let seg = attach();
        // Mark the segment for removal now: the kernel frees it once both this process and the
        // X server detach, so a crash cannot leak it.
        // SAFETY: `id` is our segment; removal is deferred until the last detach.
        unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
        match seg {
            Ok(seg) => Ok(Self {
                seg,
                addr: addr.cast(),
                size,
            }),
            Err(e) => {
                // SAFETY: `addr` was returned by `shmat` above and is detached once.
                unsafe { libc::shmdt(addr) };
                Err(e)
            }
        }
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // The server detaches when the connection closes; this process detaches here.
        // SAFETY: `addr` came from `shmat` and is detached exactly once.
        unsafe { libc::shmdt(self.addr.cast()) };
    }
}

/// MIT-SHM needs the server on this machine; `DISPLAY=host:0` means it is not.
fn is_remote_display() -> bool {
    std::env::var("DISPLAY").is_ok_and(|d| !d.starts_with(':') && !d.starts_with("unix"))
}
