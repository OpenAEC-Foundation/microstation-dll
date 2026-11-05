//! Raster scan lines (types 87 and 88; ISFF "Raster Header Element" and "Raster Data Elements",
//! §B.21): format 1 (a bit per pixel: set = foreground, clear = background), 2 (a colour index
//! per pixel) and 9 (16-bit run lengths, alternating background and foreground, background first).

use dgn_model::{Point, Raster};

/// Pixels in one image at most.
const MAX_PIXELS: usize = 1 << 26;

pub(crate) struct Image {
    width: usize,
    height: usize,
    format: u8,
    foreground: u8,
    background: u8,
    pixels: Vec<u8>,
}

impl Image {
    /// An image of the background colour; `None` for an empty, huge or unknown one.
    pub fn new(width: u32, height: u32, format: u8, foreground: u8, background: u8) -> Option<Self> {
        let (width, height) = (width as usize, height as usize);
        let size = width.checked_mul(height).filter(|&n| n > 0 && n <= MAX_PIXELS)?;
        matches!(format, 1 | 2 | 9).then(|| Image { width, height, format, foreground, background, pixels: vec![background; size] })
    }

    /// Decodes `count` pixels of row `y` (0 = top) from column `x`.
    pub fn row(&mut self, y: u32, x: u32, count: usize, data: &[u8]) {
        let (y, x) = (y as usize, x as usize);
        if y >= self.height || x >= self.width { return; }
        let (fg, bg) = (self.foreground, self.background);
        let count = count.min(self.width - x);
        let row = &mut self.pixels[y * self.width + x..(y + 1) * self.width];
        match self.format {
            2 => for (k, &c) in data.iter().take(count).enumerate() { row[k] = c },
            // The bits of a byte from the most significant one [H: the order is not documented].
            1 => for (k, pixel) in row.iter_mut().take(count).enumerate() {
                let Some(&byte) = data.get(k / 8) else { break };
                *pixel = if byte & (0x80 >> (k % 8)) != 0 { fg } else { bg };
            },
            _ => {
                let (mut k, mut on) = (0, false);
                for run in data.chunks_exact(2).map(|c| usize::from(u16::from_le_bytes([c[0], c[1]]))) {
                    let end = (k + run).min(count);
                    row[k..end].fill(if on { fg } else { bg });
                    k = end;
                    on = !on;
                    if k >= count { break; }
                }
            }
        }
    }

    pub fn finish(self, origin: Point, pixel_size: f64, transparent: bool) -> Raster {
        Raster {
            origin, pixel_size, width: self.width as u32, height: self.height as u32, format: self.format,
            transparent: transparent.then_some(self.background), pixels: self.pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decode_to_colour_indices() {
        let mut image = Image::new(10, 3, 2, 7, 0).unwrap();
        image.row(0, 0, 3, &[1, 2, 3]);
        image.row(5, 0, 3, &[9, 9, 9]);
        let mut bits = Image::new(10, 1, 1, 7, 0).unwrap();
        bits.row(0, 0, 10, &[0b1010_0000, 0b0100_0000]);
        let mut runs = Image::new(10, 1, 9, 7, 0).unwrap();
        runs.row(0, 0, 10, &[2, 0, 3, 0, 1, 0, 99, 0]);
        let raster = image.finish([0.0; 3], 1.0, true);
        assert_eq!(&raster.pixels[..4], [1, 2, 3, 0]);
        assert_eq!(raster.transparent, Some(0));
        assert_eq!(bits.finish([0.0; 3], 1.0, false).pixels, [7, 0, 7, 0, 0, 0, 0, 0, 0, 7]);
        assert_eq!(runs.finish([0.0; 3], 1.0, false).pixels, [0, 0, 7, 7, 7, 0, 7, 7, 7, 7]);
        assert!(Image::new(0, 3, 2, 0, 0).is_none() && Image::new(3, 3, 5, 0, 0).is_none() && Image::new(65535, 65535, 2, 0, 0).is_none());
    }
}
