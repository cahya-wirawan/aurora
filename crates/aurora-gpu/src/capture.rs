//! Reading back Aurora's own frame (task 160, `ui_screenshot`).
//!
//! The frame texture the app just rendered is copied into a buffer and
//! converted to RGBA8. This is a read of **Aurora's rendered frame**, not an
//! operating-system screen capture: no other window's pixels can be in it.
//! Only the 8-bit swapchain formats the app ever picks are handled (their
//! bytes already are what the display shows: sRGB-encoded); anything else
//! returns `None`.

/// The largest frame, in pixels, a capture accepts (a 16K x 16K surface is
/// 1 GiB of readback).
pub const MAX_CAPTURE_PIXELS: u64 = 64_000_000;

/// A frame copy in flight: recorded into the frame's encoder, read once the
/// encoder has been submitted.
#[derive(Debug)]
pub struct PendingCapture {
    buffer: wgpu::Buffer,
    size: (u32, u32),
    padded_row: u32,
    swap_red_blue: bool,
}

fn swap_for(format: wgpu::TextureFormat) -> Option<bool> {
    match format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => Some(true),
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(false),
        _ => None,
    }
}

/// Whether [`record_capture`] can read frames of this format.
#[must_use]
pub fn can_capture(format: wgpu::TextureFormat) -> bool {
    swap_for(format).is_some()
}

/// Records a copy of `texture` (a `format` frame of `size`) into `encoder`.
/// `texture` must have been created with `COPY_SRC` (see
/// [`crate::GpuSurface::enable_capture`]).
#[must_use]
pub fn record_capture(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Option<PendingCapture> {
    let swap_red_blue = swap_for(format)?;
    if size.0 == 0 || size.1 == 0 || u64::from(size.0) * u64::from(size.1) > MAX_CAPTURE_PIXELS {
        return None;
    }
    let padded_row = (size.0 * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aurora-frame-capture"),
        size: u64::from(padded_row) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    Some(PendingCapture {
        buffer,
        size,
        padded_row,
        swap_red_blue,
    })
}

impl PendingCapture {
    /// Waits for the copy (the encoder must have been submitted) and returns
    /// `(width, height, RGBA8 bytes)`. Blocks the calling thread for the GPU
    /// round trip.
    #[must_use]
    pub fn finish(self, device: &wgpu::Device) -> Option<(u32, u32, Vec<u8>)> {
        let slice = self.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        });
        let Ok(Ok(())) = rx.recv_timeout(std::time::Duration::from_secs(5)) else {
            return None;
        };
        let data = slice.get_mapped_range().ok()?;
        let row = self.size.0 as usize * 4;
        let mut out = Vec::with_capacity(row * self.size.1 as usize);
        for line in data.chunks_exact(self.padded_row as usize) {
            let pixels = line.get(..row)?;
            if self.swap_red_blue {
                for px in pixels.chunks_exact(4) {
                    out.extend_from_slice(&[
                        px.get(2).copied().unwrap_or(0),
                        px.get(1).copied().unwrap_or(0),
                        px.first().copied().unwrap_or(0),
                        255,
                    ]);
                }
            } else {
                for px in pixels.chunks_exact(4) {
                    out.extend_from_slice(&[
                        px.first().copied().unwrap_or(0),
                        px.get(1).copied().unwrap_or(0),
                        px.get(2).copied().unwrap_or(0),
                        255,
                    ]);
                }
            }
        }
        drop(data);
        self.buffer.unmap();
        Some((self.size.0, self.size.1, out))
    }
}
