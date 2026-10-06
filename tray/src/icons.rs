//! The embedded tray icons (drawn by `examples/make_icons.rs`). macOS uses
//! template images, which follow the menu bar's light or dark look, so the
//! states differ by shape; Linux uses coloured versions of the same shapes.
use crate::model::health::IconState;

const OK_TEMPLATE: &[u8] = include_bytes!("../assets/ok-template.png");
const ATTENTION_TEMPLATE: &[u8] = include_bytes!("../assets/attention-template.png");
const OFF_TEMPLATE: &[u8] = include_bytes!("../assets/off-template.png");
const OK: &[u8] = include_bytes!("../assets/ok.png");
const WARNING: &[u8] = include_bytes!("../assets/warning.png");
const ERROR: &[u8] = include_bytes!("../assets/error.png");
const OFF: &[u8] = include_bytes!("../assets/off.png");

/// The PNG for `state`: templates on macOS.
pub fn png(state: IconState, template: bool) -> &'static [u8] {
    match (state, template) {
        (IconState::Ok, true) => OK_TEMPLATE,
        (IconState::Warning | IconState::Error, true) => ATTENTION_TEMPLATE,
        (IconState::Off, true) => OFF_TEMPLATE,
        (IconState::Ok, false) => OK,
        (IconState::Warning, false) => WARNING,
        (IconState::Error, false) => ERROR,
        (IconState::Off, false) => OFF,
    }
}

/// RGBA pixels, width and height of an embedded PNG.
pub fn rgba(png_data: &[u8]) -> (Vec<u8>, u32, u32) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_data))
        .read_info()
        .expect("embedded icon is a PNG");
    let mut data = vec![0; reader.output_buffer_size().expect("icon size")];
    let info = reader.next_frame(&mut data).expect("embedded icon decodes");
    data.truncate(info.buffer_size());
    (data, info.width, info.height)
}

/// The icon for `state` on this platform.
pub fn icon(state: IconState) -> tray_icon::Icon {
    let (data, width, height) = rgba(png(state, cfg!(target_os = "macos")));
    tray_icon::Icon::from_rgba(data, width, height).expect("icon is RGBA")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_36_pixel_rgba_and_templates_are_black() {
        let states = [
            IconState::Ok,
            IconState::Warning,
            IconState::Error,
            IconState::Off,
        ];
        for template in [true, false] {
            for state in states {
                let (data, w, h) = rgba(png(state, template));
                assert_eq!((w, h, data.len()), (36, 36, 36 * 36 * 4));
                assert!(data.chunks(4).any(|p| p[3] > 0), "{state:?}");
                if template {
                    assert!(data.chunks(4).all(|p| p[..3] == [0, 0, 0]));
                }
            }
        }
        // Shapes differ: plain, badged and outlined.
        let shape = |s| rgba(png(s, true)).0;
        assert_ne!(shape(IconState::Ok), shape(IconState::Warning));
        assert_ne!(shape(IconState::Ok), shape(IconState::Off));
        assert_eq!(shape(IconState::Warning), shape(IconState::Error));
        assert_ne!(
            rgba(png(IconState::Warning, false)).0,
            rgba(png(IconState::Error, false)).0
        );
    }
}
