//! Pixel-preserving disintegration, inspired by Snappable's image-layer approach.
//! No encoded images or per-frame pixel processing. Each source pixel belongs to
//! exactly one dust layer or the gently fading backdrop.
use image::{imageops::FilterType, RgbaImage};
pub const LAYERS: usize = 16;
pub const MAX_PIXELS: u32 = 120_000;
pub struct Split {
    pub backdrop: RgbaImage,
    pub layers: Vec<RgbaImage>,
}
fn hash(mut n: u32) -> u32 {
    n ^= n >> 16;
    n = n.wrapping_mul(0x7feb352d);
    n ^= n >> 15;
    n = n.wrapping_mul(0x846ca68b);
    n ^ (n >> 16)
}
pub fn split(source: RgbaImage, seed: u32) -> Split {
    let pixels = source.width() as u64 * source.height() as u64;
    let source = if pixels > MAX_PIXELS as u64 {
        let ratio = (MAX_PIXELS as f64 / pixels as f64).sqrt();
        image::imageops::resize(
            &source,
            (source.width() as f64 * ratio).floor().max(1.) as u32,
            (source.height() as f64 * ratio).floor().max(1.) as u32,
            FilterType::Triangle,
        )
    } else {
        source
    };
    let (w, h) = source.dimensions();
    let mut backdrop = RgbaImage::new(w, h);
    let mut layers = (0..LAYERS)
        .map(|_| RgbaImage::new(w, h))
        .collect::<Vec<_>>();
    for (x, y, pixel) in source.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }
        // The cream card surface dissolves separately so its area does not
        // overwhelm the much smaller text, icons and image details.
        let cream = pixel[0].abs_diff(248) <= 3
            && pixel[1].abs_diff(244) <= 3
            && pixel[2].abs_diff(238) <= 3;
        if cream {
            backdrop.put_pixel(x, y, *pixel);
            continue;
        }
        // Small coherent fragments, mixed across a broad irregular wave.
        let noise = hash((x / 2).wrapping_mul(73856093) ^ (y / 2).wrapping_mul(19349663) ^ seed);
        let bucket = (((w - 1 - x) as i64 * LAYERS as i64 / w.max(1) as i64) + (noise % 9) as i64
            - 4)
        .clamp(0, LAYERS as i64 - 1) as usize;
        layers[bucket].put_pixel(x, y, *pixel);
    }
    Split { backdrop, layers }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_pixel_keeps_its_color_and_is_used_once() {
        let original = RgbaImage::from_fn(71, 39, |x, y| {
            if x < 10 {
                image::Rgba([248, 244, 238, 255])
            } else {
                image::Rgba([(x * 3) as u8, (y * 6) as u8, 125, 210])
            }
        });
        let result = split(original.clone(), 7);
        for (x, y, p) in original.enumerate_pixels() {
            let parts = std::iter::once(&result.backdrop)
                .chain(result.layers.iter())
                .filter(|i| i.get_pixel(x, y)[3] > 0)
                .collect::<Vec<_>>();
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].get_pixel(x, y), p);
        }
    }
    #[test]
    fn large_images_are_bounded_and_small_images_stay_sharp() {
        let result = split(RgbaImage::new(2400, 1200), 1);
        assert!(result.backdrop.width() * result.backdrop.height() <= MAX_PIXELS);
        assert_eq!(
            split(RgbaImage::new(40, 20), 1).backdrop.dimensions(),
            (40, 20)
        );
    }
    #[test]
    fn transparent_pixels_stay_empty_and_seed_changes_distribution() {
        let empty = split(RgbaImage::new(30, 20), 1);
        assert!(empty
            .layers
            .iter()
            .all(|l| l.as_raw().iter().all(|v| *v == 0)));
        let source = RgbaImage::from_pixel(80, 40, image::Rgba([30, 60, 90, 255]));
        let a = split(source.clone(), 1);
        let b = split(source, 2);
        assert_ne!(a.layers[5], b.layers[5]);
    }
}
