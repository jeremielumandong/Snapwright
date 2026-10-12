//! Windows backend: DXGI Desktop Duplication.
//!
//! Each monitor keeps its D3D11 device, duplication and staging texture between captures, so
//! a repeat capture is one GPU copy and one map. When the desktop has not changed since the
//! last capture, the duplication has no new frame and the last one is returned as is.

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::core::Interface;

use crate::{CaptureError, Display, Frame, Rect, Result};

fn err(api: &'static str, e: windows::core::Error) -> CaptureError {
    if e.code() == windows::Win32::Foundation::E_ACCESSDENIED {
        return CaptureError::PermissionDenied(format!("{api}: {e}"));
    }
    CaptureError::Platform {
        api,
        message: e.to_string(),
    }
}

pub(crate) struct Backend {
    outputs: Vec<Output>,
}

struct Output {
    id: u64,
    output: IDXGIOutput1,
    desc: DXGI_OUTPUT_DESC,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: Option<IDXGIOutputDuplication>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Backend {
    pub(crate) fn new() -> Result<Self> {
        // Desktop Duplication and physical-pixel coordinates both need per-monitor DPI
        // awareness. The call fails harmlessly when the app manifest already set it.
        // SAFETY: a process-wide setting with no pointers involved.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Ok(Self {
            outputs: enumerate()?,
        })
    }

    pub(crate) fn displays(&mut self) -> Result<Vec<Display>> {
        Ok(self.outputs.iter().map(Output::display).collect())
    }

    pub(crate) fn capture(&mut self, display: &Display) -> Result<Frame> {
        let Some(output) = self.outputs.iter_mut().find(|o| o.id == display.id) else {
            return Err(CaptureError::NoSuchDisplay(display.id));
        };
        match output.capture() {
            // The duplication was lost (mode change, secure desktop) and dropped: retry once
            // with a new one.
            Err(e) if output.duplication.is_none() => {
                tracing::debug!(error = %e, "duplication lost; recreating");
                output.capture()
            }
            other => other,
        }
    }
}

fn enumerate() -> Result<Vec<Output>> {
    // SAFETY: COM factory creation; the result is checked.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| err("CreateDXGIFactory1", e))?;
    let mut outputs = Vec::new();
    for a in 0u32.. {
        // SAFETY: index enumeration ends with DXGI_ERROR_NOT_FOUND.
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(a) } {
            Ok(adapter) => adapter,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(err("EnumAdapters1", e)),
        };
        let mut device_for_adapter: Option<(ID3D11Device, ID3D11DeviceContext)> = None;
        for o in 0u32.. {
            // SAFETY: as above.
            let output = match unsafe { adapter.EnumOutputs(o) } {
                Ok(output) => output,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(err("EnumOutputs", e)),
            };
            let output: IDXGIOutput1 = output.cast().map_err(|e| err("IDXGIOutput1", e))?;
            // SAFETY: GetDesc fills a plain struct.
            let desc = unsafe { output.GetDesc() }.map_err(|e| err("GetDesc", e))?;
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let (device, context) = match &device_for_adapter {
                Some(pair) => pair.clone(),
                None => {
                    let pair = create_device(&adapter)?;
                    device_for_adapter = Some(pair.clone());
                    pair
                }
            };
            outputs.push(Output {
                id: (u64::from(a) << 16) | u64::from(o),
                output,
                desc,
                device,
                context,
                duplication: None,
                staging: None,
            });
        }
    }
    if outputs.is_empty() {
        return Err(CaptureError::Unsupported(
            "no display is attached to the desktop".into(),
        ));
    }
    Ok(outputs)
}

fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: out-pointers are valid locals; the adapter outlives the call.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| err("D3D11CreateDevice", e))?;
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(CaptureError::Platform {
            api: "D3D11CreateDevice",
            message: "returned no device".into(),
        }),
    }
}

impl Output {
    fn display(&self) -> Display {
        let r = self.desc.DesktopCoordinates;
        let name_len = self
            .desc
            .DeviceName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(32);
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        // SAFETY: the monitor handle comes from DXGI; out-pointers are valid locals.
        let _ = unsafe {
            GetDpiForMonitor(self.desc.Monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
        };
        Display {
            id: self.id,
            name: String::from_utf16_lossy(&self.desc.DeviceName[..name_len]),
            bounds: Rect::new(
                r.left,
                r.top,
                (r.right - r.left).max(0) as u32,
                (r.bottom - r.top).max(0) as u32,
            ),
            scale: dpi_x as f32 / 96.0,
            primary: r.left == 0 && r.top == 0,
        }
    }

    fn capture(&mut self) -> Result<Frame> {
        let first = self.duplication.is_none();
        if first {
            // SAFETY: the device belongs to this output's adapter.
            let dup = unsafe { self.output.DuplicateOutput(&self.device) }
                .map_err(|e| err("DuplicateOutput", e))?;
            self.duplication = Some(dup);
        }
        let Some(dup) = self.duplication.clone() else {
            return Err(CaptureError::Platform {
                api: "DuplicateOutput",
                message: "no duplication".into(),
            });
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // A fresh duplication delivers the whole desktop as its first frame; later calls only
        // return when something changed, so they do not wait.
        let timeout = if first || self.staging.is_none() {
            500
        } else {
            0
        };
        // SAFETY: out-pointers are valid locals; the frame is released below on every path.
        match unsafe { dup.AcquireNextFrame(timeout, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT && self.staging.is_some() => {
                return self.read_staging();
            }
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.duplication = None;
                self.staging = None;
                return Err(err("AcquireNextFrame", e));
            }
            Err(e) => return Err(err("AcquireNextFrame", e)),
        }
        let copied = resource
            .ok_or_else(|| CaptureError::Platform {
                api: "AcquireNextFrame",
                message: "no frame resource".into(),
            })
            .and_then(|r| {
                r.cast::<ID3D11Texture2D>()
                    .map_err(|e| err("ID3D11Texture2D", e))
            })
            .and_then(|texture| self.copy_to_staging(&texture));
        // SAFETY: a frame was acquired above.
        let released = unsafe { dup.ReleaseFrame() }.map_err(|e| err("ReleaseFrame", e));
        copied?;
        released?;
        self.read_staging()
    }

    fn copy_to_staging(&mut self, texture: &ID3D11Texture2D) -> Result<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: fills a plain struct.
        unsafe { texture.GetDesc(&mut desc) };
        let reuse =
            matches!(&self.staging, Some((_, w, h)) if *w == desc.Width && *h == desc.Height);
        if !reuse {
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            let mut staging = None;
            // SAFETY: the descriptor is valid for a CPU-readable copy of the frame.
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut staging)) }
                .map_err(|e| err("CreateTexture2D", e))?;
            let staging = staging.ok_or_else(|| CaptureError::Platform {
                api: "CreateTexture2D",
                message: "returned no texture".into(),
            })?;
            self.staging = Some((staging, desc.Width, desc.Height));
        }
        if let Some((staging, _, _)) = &self.staging {
            // SAFETY: both textures share size and format and live on this device.
            unsafe { self.context.CopyResource(staging, texture) };
        }
        Ok(())
    }

    fn read_staging(&self) -> Result<Frame> {
        let Some((staging, width, height)) = &self.staging else {
            return Err(CaptureError::Platform {
                api: "Map",
                message: "nothing captured yet".into(),
            });
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU-readable and unmapped below.
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| err("Map", e))?;
        let stride = mapped.RowPitch as usize;
        // SAFETY: a mapped texture exposes `RowPitch * height` readable bytes at `pData`.
        let bytes = unsafe {
            std::slice::from_raw_parts(mapped.pData as *const u8, stride * *height as usize)
        };
        let frame = Frame::from_bgrx_rows(self.id, *width, *height, stride, bytes);
        // SAFETY: mapped above.
        unsafe { self.context.Unmap(staging, 0) };
        Ok(frame)
    }
}
