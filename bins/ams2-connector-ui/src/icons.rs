//! The connector's icons, drawn in code (no image files): an "A" on a disc whose colour is the
//! level of the connector (off = grey, ok = green, warn = amber, error = red). They are our
//! own artwork; nothing is copied from a
//! third party.
//!
//! Pure standard library, so `build.rs` includes this file to write the executables' `.ico`.

/// The colour of a level word (`off`, `ok`, `warn`, `error`; anything else is `off`).
pub fn level_rgb(level: &str) -> [u8; 3] {
    match level {
        "ok" => [0x2e, 0x9d, 0x48],
        "warn" => [0xe0, 0xa1, 0x00],
        "error" => [0xd1, 0x34, 0x38],
        _ => [0x8a, 0x8a, 0x8a],
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * abx, a.1 + t * aby);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Whether a point of the unit square is on the glyph, and whether it is on the disc.
fn shape_at(p: (f32, f32)) -> (bool, bool) {
    let (dx, dy) = (p.0 - 0.5, p.1 - 0.5);
    let disc = (dx * dx + dy * dy).sqrt() <= 0.47;
    let half = 0.07;
    let apex = (0.5, 0.2);
    let strokes = [
        (apex, (0.29, 0.8)),
        (apex, (0.71, 0.8)),
        ((0.375, 0.6), (0.625, 0.6)),
    ];
    let glyph = strokes
        .iter()
        .any(|&(a, b)| segment_distance(p, a, b) <= half);
    (glyph, disc)
}

/// RGBA pixels (row by row) of the icon of `level`, `size` pixels square, 4x4 supersampled.
pub fn render_rgba(level: &str, size: u32) -> Vec<u8> {
    let [r, g, b] = level_rgb(level);
    let n = 4u32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut disc_sum, mut glyph_sum) = (0.0f32, 0.0f32);
            for sy in 0..n {
                for sx in 0..n {
                    let p = (
                        (x as f32 + (sx as f32 + 0.5) / n as f32) / size as f32,
                        (y as f32 + (sy as f32 + 0.5) / n as f32) / size as f32,
                    );
                    let (glyph, disc) = shape_at(p);
                    if disc {
                        disc_sum += 1.0;
                        if glyph {
                            glyph_sum += 1.0;
                        }
                    }
                }
            }
            let alpha = disc_sum / (n * n) as f32;
            let white = if disc_sum > 0.0 {
                glyph_sum / disc_sum
            } else {
                0.0
            };
            let mix = |c: u8| (f32::from(c) * (1.0 - white) + 255.0 * white).round() as u8;
            out.extend_from_slice(&[mix(r), mix(g), mix(b), (alpha * 255.0).round() as u8]);
        }
    }
    out
}

/// A Windows `.ico` file with 32-bit images of `sizes` (each at most 255).
pub fn ico_bytes(level: &str, sizes: &[u32]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend_from_slice(&[0, 0, 1, 0]);
    file.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut images: Vec<Vec<u8>> = Vec::new();
    for &s in sizes {
        let rgba = render_rgba(level, s);
        let mut dib = Vec::new();
        // BITMAPINFOHEADER: the height counts the XOR and the AND masks.
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(s as i32).to_le_bytes());
        dib.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                dib.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        let row = (s.div_ceil(32) * 4) as usize;
        dib.extend(std::iter::repeat_n(0u8, row * s as usize));
        images.push(dib);
    }
    let mut offset = 6 + 16 * sizes.len();
    for (&s, img) in sizes.iter().zip(&images) {
        file.extend_from_slice(&[s as u8, s as u8, 0, 0, 1, 0, 32, 0]);
        file.extend_from_slice(&(img.len() as u32).to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in images {
        file.extend_from_slice(&img);
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_disc_with_a_white_glyph() {
        let px = render_rgba("ok", 32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "outside the disc");
        assert_eq!(&at(5, 16)[..3], &level_rgb("ok"), "on the disc");
        assert_eq!(at(5, 16)[3], 255);
        assert_eq!(&at(16, 19)[..3], &[255, 255, 255], "the crossbar of the A");
    }

    #[test]
    fn the_levels_have_different_colours() {
        let all: Vec<_> = ["off", "ok", "warn", "error"].map(level_rgb).to_vec();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(level_rgb("nonsense"), level_rgb("off"));
    }

    #[test]
    fn the_ico_has_a_directory_and_its_images() {
        let ico = ico_bytes("ok", &[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        let size = u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize;
        let off = u32::from_le_bytes(ico[18..22].try_into().unwrap()) as usize;
        assert_eq!(off, 6 + 32);
        assert_eq!(size, 40 + 16 * 16 * 4 + 4 * 16);
    }
}
