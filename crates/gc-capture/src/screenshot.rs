//! Screenshots straight from the GPU framebuffer via DXGI Desktop
//! Duplication: the duplicated desktop texture is copied into a CPU-readable
//! staging texture once and encoded to PNG/JPEG — no GDI BitBlt, no hooks.
#![cfg_attr(not(windows), allow(dead_code))]

use gc_core::config::ImageFormat;
use std::path::Path;

/// What to grab.
#[derive(Debug, Clone, Copy)]
pub struct ShotTarget {
    pub adapter_idx: u32,
    pub output_idx: u32,
    /// Optional crop `(x, y, w, h)` relative to the output.
    pub region: Option<(i32, i32, u32, u32)>,
}

pub fn encode_rgba(
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    format: ImageFormat,
    jpeg_quality: u8,
    out: &Path,
) -> anyhow::Result<()> {
    let img = image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| anyhow::anyhow!("bad image buffer"))?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    match format {
        ImageFormat::Png => {
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut { file }, image::ImageFormat::Png)?;
        }
        ImageFormat::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, jpeg_quality);
            enc.encode_image(&rgb)?;
        }
    }
    Ok(())
}

/// Converts a BGRA texture mapping (with row pitch) into tightly packed
/// RGBA, cropping to `region`.
pub fn bgra_to_rgba(
    src: &[u8],
    pitch: usize,
    width: u32,
    height: u32,
    region: Option<(i32, i32, u32, u32)>,
) -> (Vec<u8>, u32, u32) {
    let (x0, y0, w, h) = match region {
        Some((x, y, w, h)) => {
            let x = x.clamp(0, width as i32) as u32;
            let y = y.clamp(0, height as i32) as u32;
            (x, y, w.min(width - x), h.min(height - y))
        }
        None => (0, 0, width, height),
    };
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y0..y0 + h {
        let start = row as usize * pitch + x0 as usize * 4;
        for px in src[start..start + w as usize * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    (out, w, h)
}

#[cfg(not(windows))]
pub fn capture(
    _t: ShotTarget,
    _format: ImageFormat,
    _q: u8,
    _out: &Path,
) -> anyhow::Result<(u32, u32)> {
    anyhow::bail!("screenshots are only supported on Windows")
}

#[cfg(windows)]
pub fn capture(
    t: ShotTarget,
    format: ImageFormat,
    jpeg_quality: u8,
    out: &Path,
) -> anyhow::Result<(u32, u32)> {
    let (rgba, w, h) = unsafe { imp::grab(t)? };
    encode_rgba(rgba, w, h, format, jpeg_quality, out)?;
    Ok((w, h))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
        D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIResource,
        DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    };

    pub unsafe fn grab(t: ShotTarget) -> anyhow::Result<(Vec<u8>, u32, u32)> {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter = factory.EnumAdapters1(t.adapter_idx)?;
        let output: IDXGIOutput1 = adapter.EnumOutputs(t.output_idx)?.cast()?;

        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            &adapter.cast::<IDXGIAdapter>()?,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
        let device = device.ok_or_else(|| anyhow::anyhow!("no D3D11 device"))?;
        let ctx = ctx.ok_or_else(|| anyhow::anyhow!("no D3D11 context"))?;
        let dupl = output.DuplicateOutput(&device)?;

        // The first frames after DuplicateOutput may be empty; retry briefly.
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        for attempt in 0..10 {
            match dupl.AcquireNextFrame(100, &mut info, &mut resource) {
                Ok(()) if info.LastPresentTime != 0 || attempt >= 3 => break,
                Ok(()) => {
                    resource = None;
                    let _ = dupl.ReleaseFrame();
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT && attempt >= 3 => break,
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(e) => return Err(e.into()),
            }
        }
        let resource =
            resource.ok_or_else(|| anyhow::anyhow!("desktop duplication produced no frame"))?;
        let tex: ID3D11Texture2D = resource.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        anyhow::ensure!(
            desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM,
            "unsupported desktop format {:?} (HDR?)",
            desc.Format
        );
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            MipLevels: 1,
            ArraySize: 1,
            ..desc
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| anyhow::anyhow!("staging texture"))?;
        ctx.CopyResource(&staging, &tex);
        let _ = dupl.ReleaseFrame();

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let len = mapped.RowPitch as usize * desc.Height as usize;
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, len);
        let result = bgra_to_rgba(
            src,
            mapped.RowPitch as usize,
            desc.Width,
            desc.Height,
            t.region,
        );
        ctx.Unmap(&staging, 0);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_crops() {
        // 3x2 BGRA with pitch 16 (4 bytes padding per row)
        let mut src = vec![0u8; 32];
        for y in 0..2 {
            for x in 0..3 {
                let i = y * 16 + x * 4;
                src[i..i + 4].copy_from_slice(&[x as u8, y as u8, 200, 0]);
            }
        }
        let (rgba, w, h) = bgra_to_rgba(&src, 16, 3, 2, None);
        assert_eq!((w, h), (3, 2));
        assert_eq!(&rgba[0..4], &[200, 0, 0, 255]);
        let (rgba, w, h) = bgra_to_rgba(&src, 16, 3, 2, Some((1, 1, 10, 10)));
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![200, 1, 1, 255, 200, 1, 2, 255]);

        let dir = tempfile::tempdir().unwrap();
        for f in [ImageFormat::Png, ImageFormat::Jpeg] {
            let p = dir.path().join(format!("s.{}", f.extension()));
            encode_rgba(vec![128; 16 * 16 * 4], 16, 16, f, 90, &p).unwrap();
            assert!(image::open(&p).is_ok());
        }
    }
}
