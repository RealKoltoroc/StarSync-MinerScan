#![cfg(target_os = "windows")]

use anyhow::{Context, Result, anyhow, bail};
use image::RgbaImage;
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice,
                ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
            },
            Dxgi::{
                DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1,
                IDXGIOutputDuplication, IDXGIResource,
            },
            Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint},
        },
    },
    core::Interface,
};

use windows::Win32::Foundation::POINT;

use crate::models::CaptureRegion;

/// Persistent, monitor-scoped DXGI Desktop Duplication capture.
///
/// Unlike the xcap full-monitor video recorder this copies only the configured
/// OCR ROI from the GPU texture into CPU-visible staging memory. A typical
/// 230x74 scan area therefore transfers ~68 KiB rather than tens of MiB per
/// OCR attempt. Desktop Duplication also sees DirectX/flip-model game output
/// that GDI/BitBlt can miss.
pub struct DxgiRoiCapture {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
}

impl DxgiRoiCapture {
    pub fn new(monitor_x: i32, monitor_y: i32) -> Result<Self> {
        let point = POINT {
            x: monitor_x + 1,
            y: monitor_y + 1,
        };
        let h_monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) };
        if h_monitor.is_invalid() {
            bail!("DXGI could not resolve the selected monitor");
        }

        let d3d_device = create_d3d_device()?;
        let d3d_context = unsafe { d3d_device.GetImmediateContext()? };
        let dxgi_device = d3d_device.cast::<IDXGIDevice>()?;
        let adapter = unsafe { dxgi_device.GetAdapter()? };

        let mut output_index = 0;
        loop {
            let output = unsafe { adapter.EnumOutputs(output_index) }
                .map_err(|_| anyhow!("DXGI selected monitor is not available on the active GPU"))?;
            output_index += 1;
            let output_desc = unsafe { output.GetDesc()? };
            if output_desc.Monitor != h_monitor {
                continue;
            }
            let output1 = output.cast::<IDXGIOutput1>()?;
            let duplication = unsafe { output1.DuplicateOutput(&dxgi_device)? };
            return Ok(Self {
                d3d_device,
                d3d_context,
                duplication,
            });
        }
    }

    pub fn capture(&self, region: CaptureRegion, timeout_ms: u32) -> Result<RgbaImage> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let acquired = unsafe {
            match self
                .duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
            {
                Ok(()) => true,
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    bail!("DXGI capture timed out")
                }
                Err(error) => return Err(error).context("DXGI AcquireNextFrame failed"),
            }
        };

        debug_assert!(acquired);
        let result = self.capture_acquired(resource, frame_info, region);
        let release = unsafe { self.duplication.ReleaseFrame() };
        if let Err(error) = release {
            if result.is_ok() {
                return Err(error).context("DXGI ReleaseFrame failed");
            }
        }
        result
    }

    fn capture_acquired(
        &self,
        resource: Option<IDXGIResource>,
        frame_info: DXGI_OUTDUPL_FRAME_INFO,
        region: CaptureRegion,
    ) -> Result<RgbaImage> {
        if frame_info.LastPresentTime == 0 {
            bail!("DXGI frame contained no newly presented image");
        }
        let source_texture = resource
            .context("DXGI frame did not contain a texture")?
            .cast::<ID3D11Texture2D>()?;
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { source_texture.GetDesc(&mut source_desc) };

        let x = region.x.max(0) as u32;
        let y = region.y.max(0) as u32;
        let width = region.width.max(20);
        let height = region.height.max(20);
        if x + width > source_desc.Width || y + height > source_desc.Height {
            bail!(
                "OCR region ({x},{y},{width},{height}) exceeds DXGI frame {}x{}",
                source_desc.Width,
                source_desc.Height
            );
        }

        let mut staging_desc = source_desc;
        staging_desc.Width = width;
        staging_desc.Height = height;
        staging_desc.BindFlags = 0;
        staging_desc.MiscFlags = 0;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        staging_desc.MipLevels = 1;
        staging_desc.ArraySize = 1;

        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            self.d3d_device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        }
        let staging = staging.context("DXGI could not allocate ROI staging texture")?;
        let source_resource: ID3D11Resource = source_texture.cast()?;
        let staging_resource: ID3D11Resource = staging.cast()?;
        let box_region = D3D11_BOX {
            left: x,
            top: y,
            front: 0,
            right: x + width,
            bottom: y + height,
            back: 1,
        };
        unsafe {
            self.d3d_context.CopySubresourceRegion(
                &staging_resource,
                0,
                0,
                0,
                0,
                &source_resource,
                0,
                Some(&box_region),
            );
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.d3d_context
                .Map(&staging_resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }

        let mut rgba = vec![0_u8; (width * height * 4) as usize];
        let src_ptr = mapped.pData as *const u8;
        for row in 0..height {
            let src_offset = (row * mapped.RowPitch) as usize;
            let dst_offset = (row * width * 4) as usize;
            let src = unsafe {
                std::slice::from_raw_parts(src_ptr.add(src_offset), (width * 4) as usize)
            };
            let dst = &mut rgba[dst_offset..dst_offset + (width * 4) as usize];
            dst.copy_from_slice(src);
            // Desktop Duplication surfaces are BGRA. OCR/image expects RGBA.
            for pixel in dst.chunks_exact_mut(4) {
                pixel.swap(0, 2);
                if pixel[3] == 0 {
                    pixel[3] = 255;
                }
            }
        }
        unsafe { self.d3d_context.Unmap(&staging_resource, 0) };

        RgbaImage::from_raw(width, height, rgba).context("DXGI ROI produced an invalid RGBA buffer")
    }
}

fn create_d3d_device() -> Result<ID3D11Device> {
    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.context("D3D11CreateDevice returned no device")
}
