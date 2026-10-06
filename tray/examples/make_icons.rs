//! Draws the tray icons into `tray/assets/`: an envelope, plain (ok), with
//! a `!` badge (warning, error) and as an outline (off). Run once with
//! `cargo run -p mailtriage-tray --example make_icons` and commit the PNGs;
//! the tray embeds them.
use std::{fs::File, io::BufWriter, path::Path};

const SIZE: u32 = 36;
const SAMPLES: u32 = 4;

fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
}

/// Distance from (x, y) to the segment a–b.
fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    dist(x, y, a.0 + t * dx, a.1 + t * dy)
}

fn body(x: f32, y: f32) -> bool {
    (3.0..=33.0).contains(&x) && (8.0..=28.0).contains(&y)
}

fn flap(x: f32, y: f32) -> bool {
    segment(x, y, (3.0, 8.0), (18.0, 19.0)) < 1.4 || segment(x, y, (18.0, 19.0), (33.0, 8.0)) < 1.4
}

fn outline(x: f32, y: f32) -> bool {
    body(x, y) && !((5.5..=30.5).contains(&x) && (10.5..=25.5).contains(&y))
}

fn ok(x: f32, y: f32) -> bool {
    body(x, y) && !flap(x, y)
}

fn off(x: f32, y: f32) -> bool {
    outline(x, y) || (flap(x, y) && body(x, y))
}

fn attention(x: f32, y: f32) -> bool {
    let bang =
        ((26.9..=29.1).contains(&x) && (3.5..=10.0).contains(&y)) || dist(x, y, 28.0, 12.8) < 1.2;
    (ok(x, y) && dist(x, y, 28.0, 8.5) >= 9.5) || (dist(x, y, 28.0, 8.5) < 7.5 && !bang)
}

fn draw(name: &str, shape: fn(f32, f32) -> bool, rgb: [u8; 3]) {
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = px as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                    hits += u32::from(shape(x, y));
                }
            }
            let alpha = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(&path).unwrap()), SIZE, SIZE);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&data)
        .unwrap();
    println!("{}", path.display());
}

fn main() {
    // macOS template images: black, only the alpha channel counts.
    draw("ok-template.png", ok, [0, 0, 0]);
    draw("attention-template.png", attention, [0, 0, 0]);
    draw("off-template.png", off, [0, 0, 0]);
    // Linux: the same shapes in colour.
    draw("ok.png", ok, [0x2e, 0x7d, 0x32]);
    draw("warning.png", attention, [0xf2, 0x99, 0x00]);
    draw("error.png", attention, [0xc6, 0x28, 0x28]);
    draw("off.png", off, [0x75, 0x75, 0x75]);
}
