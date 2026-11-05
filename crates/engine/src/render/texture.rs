//! Point-sampled textures. The grid texture is generated in code: a 2×2 m
//! checker with a dark line every meter and faint lines every 25 cm, so
//! floors and walls give a sense of scale and speed.

/// Texels per meter of the grid texture.
pub const GRID_TEXELS_PER_METER: u32 = 32;

/// Meters covered by one repeat of the grid texture (the shader needs this).
pub const GRID_METERS: u32 = 2;

/// An RGBA8 image in sRGB, with its mip chain (level 0 first).
pub struct Image {
    pub size: u32,
    pub mips: Vec<Vec<u8>>,
}

pub fn grid_image() -> Image {
    let size = GRID_TEXELS_PER_METER * GRID_METERS;
    let quarter = GRID_TEXELS_PER_METER / 4;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let cell = (x / GRID_TEXELS_PER_METER + y / GRID_TEXELS_PER_METER) % 2;
            let mut value: u8 = if cell == 0 { 150 } else { 132 };
            if x % quarter == 0 || y % quarter == 0 {
                value -= 14;
            }
            if x % GRID_TEXELS_PER_METER == 0 || y % GRID_TEXELS_PER_METER == 0 {
                value = 70;
            }
            pixels.extend([value, value, value, 255]);
        }
    }
    Image {
        size,
        mips: mip_chain(pixels, size),
    }
}

/// Box-filters each level down to 1×1. Averaging happens in linear light,
/// otherwise distant grid lines get too dark.
fn mip_chain(level0: Vec<u8>, size: u32) -> Vec<Vec<u8>> {
    let mut mips = vec![level0];
    let mut size = size;
    while size > 1 {
        let prev = mips.last().expect("level 0 exists");
        let half = size / 2;
        let mut next = Vec::with_capacity((half * half * 4) as usize);
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let texel = |dx: u32, dy: u32| {
                        let i = (((2 * y + dy) * size + 2 * x + dx) * 4 + c) as usize;
                        prev[i]
                    };
                    let samples = [texel(0, 0), texel(1, 0), texel(0, 1), texel(1, 1)];
                    next.push(if c == 3 {
                        (samples.iter().map(|&s| s as u32).sum::<u32>() / 4) as u8
                    } else {
                        let linear = samples.iter().map(|&s| srgb_to_linear(s)).sum::<f32>() / 4.0;
                        linear_to_srgb(linear)
                    });
                }
            }
        }
        mips.push(next);
        size = half;
    }
    mips
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = value as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(linear: f32) -> u8 {
    let c = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Uploads an image as an sRGB texture with all its mips.
pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, image: &Image) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grid"),
        size: wgpu::Extent3d {
            width: image.size,
            height: image.size,
            depth_or_array_layers: 1,
        },
        mip_level_count: image.mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, data) in image.mips.iter().enumerate() {
        let size = (image.size >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_goes_down_to_one_texel() {
        let image = grid_image();
        assert_eq!(image.size, 64);
        assert_eq!(image.mips.len(), 7); // 64, 32, 16, 8, 4, 2, 1
        for (level, data) in image.mips.iter().enumerate() {
            let size = (image.size >> level) as usize;
            assert_eq!(data.len(), size * size * 4);
        }
    }

    #[test]
    fn srgb_round_trips() {
        for v in [0u8, 1, 70, 128, 200, 255] {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
        }
    }
}
