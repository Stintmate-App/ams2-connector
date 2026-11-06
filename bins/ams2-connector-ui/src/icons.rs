//! The connector's icon: the Stintmate artwork in `assets/` (`icon_32.png`, `icon_64.png`; the
//! executables' resource is `assets/icon.ico`, set by each `build.rs`). The tray icon carries the
//! state of the connector as a badge in the lower right corner, whose colour is the level
//! (off = grey, ok = green, warn = amber, error = red); the window's icon is the artwork alone.

/// Size of the tray icon, in pixels.
pub const TRAY_SIZE: u32 = 32;
/// Size of the window icon, in pixels.
pub const WINDOW_SIZE: u32 = 64;

static ICON_32: &[u8] = include_bytes!("../assets/icon_32.png");
static ICON_64: &[u8] = include_bytes!("../assets/icon_64.png");

/// The colour of a level word (`off`, `ok`, `warn`, `error`; anything else is `off`).
pub fn level_rgb(level: &str) -> [u8; 3] {
    match level {
        "ok" => [0x2e, 0x9d, 0x48],
        "warn" => [0xe0, 0xa1, 0x00],
        "error" => [0xd1, 0x34, 0x38],
        _ => [0x8a, 0x8a, 0x8a],
    }
}

/// RGBA pixels (row by row) of the artwork, `size` pixels square: [`TRAY_SIZE`] or
/// [`WINDOW_SIZE`].
///
/// # Panics
/// On any other size, or if the embedded image is not an 8-bit RGBA PNG (a build error).
pub fn artwork_rgba(size: u32) -> Vec<u8> {
    let bytes = match size {
        TRAY_SIZE => ICON_32,
        WINDOW_SIZE => ICON_64,
        other => panic!("no artwork of {other} px"),
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .expect("the embedded icon is a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("the icon's size")];
    let info = reader
        .next_frame(&mut buf)
        .expect("the embedded icon decodes");
    assert!(
        info.width == size
            && info.height == size
            && info.color_type == png::ColorType::Rgba
            && info.bit_depth == png::BitDepth::Eight,
        "the embedded icon is {size}x{size} RGBA8"
    );
    buf.truncate(info.buffer_size());
    buf
}

/// The artwork with the badge of `level` in the lower right corner (a disc of the level's colour
/// inside a dark ring, anti-aliased), `size` as in [`artwork_rgba`].
pub fn render_rgba(level: &str, size: u32) -> Vec<u8> {
    let mut px = artwork_rgba(size);
    let [r, g, b] = level_rgb(level);
    let s = size as f32;
    let (cx, cy) = (0.74 * s, 0.74 * s);
    let disc = 0.21 * s;
    let ring = disc + (0.06 * s).max(1.0);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let ring_a = (ring - d + 0.5).clamp(0.0, 1.0);
            if ring_a == 0.0 {
                continue;
            }
            let disc_a = (disc - d + 0.5).clamp(0.0, 1.0);
            // The badge's own colour and coverage: ring colour under the disc colour.
            let ring_rgb = [0x1b_u8, 0x1b, 0x1b];
            let mix = |i: usize, c: u8| {
                (f32::from(ring_rgb[i]) * (1.0 - disc_a) + f32::from(c) * disc_a).round()
            };
            let badge = [mix(0, r), mix(1, g), mix(2, b)];
            let i = ((y * size + x) * 4) as usize;
            let under = f32::from(px[i + 3]) / 255.0;
            let out_a = ring_a + under * (1.0 - ring_a);
            for k in 0..3 {
                let c = (badge[k] * ring_a + f32::from(px[i + k]) * under * (1.0 - ring_a)) / out_a;
                px[i + k] = c.round() as u8;
            }
            px[i + 3] = (out_a * 255.0).round() as u8;
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_artwork_decodes_at_both_sizes() {
        for size in [TRAY_SIZE, WINDOW_SIZE] {
            let px = artwork_rgba(size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert!(px.chunks(4).any(|p| p[3] > 0), "not blank");
        }
    }

    #[test]
    fn the_badge_shows_the_level_and_leaves_the_rest_alone() {
        let size = TRAY_SIZE;
        let base = artwork_rgba(size);
        let at = |px: &[u8], x: u32, y: u32| px[((y * size + x) * 4) as usize..][..4].to_vec();
        for level in ["off", "ok", "warn", "error"] {
            let px = render_rgba(level, size);
            let c = (0.74 * size as f32) as u32;
            assert_eq!(
                &at(&px, c, c)[..3],
                &level_rgb(level),
                "{level}: badge centre"
            );
            assert_eq!(at(&px, c, c)[3], 255);
            assert_eq!(
                at(&px, 1, 1),
                at(&base, 1, 1),
                "{level}: far corner untouched"
            );
        }
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
    fn the_ico_resource_has_the_standard_sizes() {
        let ico = include_bytes!("../assets/icon.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        let n = u16::from_le_bytes([ico[4], ico[5]]);
        assert!(n >= 4, "{n} images");
    }
}
