//! Pixel adjustments for a comic page. Values are applied to RGBA bytes.

use thiserror::Error;

/// Why image bytes were not adjusted.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdjustError {
    /// The buffer length is not a multiple of four, or `width` does not divide the pixel count.
    #[error("image layout does not match rgba pixels")]
    Layout,
}

/// Adjustments in the range -100..=100, plus invert and sharpen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImageAdjust {
    /// Added to each colour channel.
    pub brightness: i32,
    /// Contrast percent added to 100. Zero leaves the channel alone.
    pub contrast: i32,
    /// Saturation percent added to 100. Zero leaves the channel alone.
    pub saturation: i32,
    /// Red up and blue down.
    pub temperature: i32,
    /// Replace each colour with `255 - channel`.
    pub invert: bool,
    /// A one-pixel unsharp step toward the next pixel.
    pub sharpen: bool,
}

/// Apply `adjust` to tightly packed RGBA pixels of `width` columns.
///
/// # Errors
///
/// Returns an error when the buffer is not whole pixels or `width` does not tile it.
pub fn apply_rgba(pixels: &mut [u8], width: u32, adjust: &ImageAdjust) -> Result<(), AdjustError> {
    if pixels.len() % 4 != 0 {
        return Err(AdjustError::Layout);
    }
    let count = pixels.len() / 4;
    let columns = usize::try_from(width).map_err(|_| AdjustError::Layout)?;
    if columns == 0 || count % columns != 0 {
        return Err(AdjustError::Layout);
    }
    let brightness = adjust.brightness.clamp(-100, 100);
    let contrast = adjust.contrast.clamp(-100, 100);
    let saturation = adjust.saturation.clamp(-100, 100);
    let temperature = adjust.temperature.clamp(-100, 100);
    // to_vec: sharpen reads the unadjusted neighbour
    let source = pixels.to_vec();
    for index in 0..count {
        let start = index.saturating_mul(4);
        let mut rgb = [0i32; 3];
        for (channel, value) in rgb.iter_mut().enumerate() {
            let original = i32::from(source[start.saturating_add(channel)]);
            let colour = if adjust.invert {
                255 - original
            } else {
                original
            };
            let shifted = colour.saturating_add(brightness);
            *value = 128 + (shifted - 128) * (100 + contrast) / 100;
        }
        let gray = (rgb[0] + rgb[1] + rgb[2]) / 3;
        for value in &mut rgb {
            *value = gray + (*value - gray) * (100 + saturation) / 100;
        }
        rgb[0] = rgb[0].saturating_add(temperature);
        rgb[2] = rgb[2].saturating_sub(temperature);
        if adjust.sharpen {
            for (channel, value) in rgb.iter_mut().enumerate() {
                *value = value
                    .saturating_mul(2)
                    .saturating_sub(neighbour(&source, index, count, channel));
            }
        }
        for (channel, value) in rgb.iter().enumerate() {
            pixels[start.saturating_add(channel)] = clamp_channel(*value);
        }
    }
    Ok(())
}

fn neighbour(source: &[u8], index: usize, count: usize, channel: usize) -> i32 {
    let next = if index + 1 < count { index + 1 } else { index };
    let at = next.saturating_mul(4).saturating_add(channel);
    i32::from(source[at])
}

fn clamp_channel(value: i32) -> u8 {
    if value <= 0 {
        0
    } else if value >= 255 {
        255
    } else {
        u8::try_from(value).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_rgba, ImageAdjust};

    #[test]
    fn adjust_changes_rgba_bytes() {
        let mut pixels = [10u8, 20, 30, 255];
        apply_rgba(&mut pixels, 1, &ImageAdjust::default()).expect("identity");
        assert_eq!(pixels, [10, 20, 30, 255]);
        apply_rgba(
            &mut pixels,
            1,
            &ImageAdjust {
                invert: true,
                ..ImageAdjust::default()
            },
        )
        .expect("invert");
        assert_eq!(pixels, [245, 235, 225, 255]);
        let mut bright = [0u8, 0, 0, 255];
        apply_rgba(
            &mut bright,
            1,
            &ImageAdjust {
                brightness: 5,
                ..ImageAdjust::default()
            },
        )
        .expect("bright");
        assert_eq!(bright[0], 5);
    }
}
