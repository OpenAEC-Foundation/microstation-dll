//! Reader of MicroStation design files (`.dgn`) into the neutral [`dgn_model::Design`].
//!
//! - **V7** (IGDS and MicroStation up to version 7): written from Intergraph's public
//!   "Intergraph Standard File Formats (Element Structure)" (ISFF) and the clean-room
//!   specification of the DGN analysis (`SPEC_v0.1.md` §A: design file header, colour table,
//!   quaternion layout, measured on sample files).
//! - **V8** (V8, V8i, CONNECT and later): an OLE2 compound file holding zlib-compressed element
//!   streams, written only from the clean-room specification (`SPEC_v0.1.md` §B) and Microsoft's
//!   public [MS-CFB] description of the compound file.
//!
//! What the specification does not describe yet (V8 dimensions other than 2D linear ones,
//! multi-lines, tags, raster, meshes, Parasolid solids, further models) is skipped and counted in
//! [`dgn_model::Design::skipped`].
//!
//! ```no_run
//! let design = dgn_read::read_file("drawing.dgn").unwrap();
//! for element in &design.models[0].elements {
//!     println!("{} on {}", dgn_model::kind_name(element.kind), design.level_name(element.level));
//! }
//! ```

use std::collections::BTreeMap;

use dgn_model::Design;
use glam::{DMat3, DQuat};

pub mod cfb;
pub mod v7;
pub mod v8;
mod raster;
mod zlib;

#[cfg(any(test, feature = "synth"))]
pub mod synth;

pub use zlib::inflate;

/// Elements read at most from one file.
pub const MAX_ELEMENTS: usize = 2_000_000;
/// Complex elements nested deeper than this are skipped: a safety net, real files nest at most
/// 4 deep (§B.3).
pub const MAX_DEPTH: usize = 64;
/// The OLE2 compound file signature that starts every V8 file.
pub const OLE_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// Fixed-point unit of V7 quaternions, B-spline knots and weights: 2^31 - 1.
pub(crate) const UNIT: f64 = 2_147_483_647.0;

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

impl std::error::Error for Error {}

impl From<Error> for String {
    fn from(e: Error) -> String { e.0 }
}

impl From<&str> for Error {
    fn from(s: &str) -> Error { Error(s.to_string()) }
}

impl From<String> for Error {
    fn from(s: String) -> Error { Error(s) }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Reads a whole design file: V8 files are compound files, everything else is read as V7.
pub fn read(data: &[u8]) -> Result<Design> {
    if data.starts_with(&OLE_SIGNATURE) { v8::read(data) } else { v7::read(data) }
}

/// Reads a design file from disk.
pub fn read_file(path: impl AsRef<std::path::Path>) -> Result<Design> {
    let path = path.as_ref();
    let data = std::fs::read(path).map_err(|e| Error(format!("MicroStation DGN: cannot read {}: {e}", path.display())))?;
    read(&data)
}

/// Counts of what was not read, by what it is.
#[derive(Default)]
pub(crate) struct Skipped(pub BTreeMap<String, usize>);

impl Skipped {
    pub fn add(&mut self, what: &str) {
        match self.0.get_mut(what) {
            Some(count) => *count += 1,
            None => { self.0.insert(what.to_string(), 1); }
        }
    }
}

pub(crate) fn le16(data: &[u8], at: usize) -> u16 { data.get(at..at.saturating_add(2)).map_or(0, |b| u16::from_le_bytes([b[0], b[1]])) }
pub(crate) fn le32(data: &[u8], at: usize) -> u32 { data.get(at..at.saturating_add(4)).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) }
pub(crate) fn le64(data: &[u8], at: usize) -> u64 { u64::from(le32(data, at)) | (u64::from(le32(data, at.saturating_add(4))) << 32) }
/// An IEEE double; NaN outside the data, so a short element gives a non-finite value.
pub(crate) fn lef64(data: &[u8], at: usize) -> f64 {
    if at.checked_add(8).is_some_and(|end| end <= data.len()) { f64::from_bits(le64(data, at)) } else { f64::NAN }
}

pub(crate) fn latin1(bytes: &[u8]) -> String { bytes.iter().map(|&c| char::from(c)).collect() }

/// The rotation of a stored quaternion (w, x, y, z); the columns are the element's axes in design
/// coordinates. The file stores the inverse rotation: a text turned +30 degrees about Z has
/// (cos 15, 0, 0, -sin 15).
pub(crate) fn quaternion(w: f64, x: f64, y: f64, z: f64) -> DMat3 {
    let q = DQuat::from_xyzw(-x, -y, -z, w);
    if q.length_squared() >= 1e-12 { DMat3::from_quat(q.normalize()) } else { DMat3::IDENTITY }
}

/// A full knot vector for `poles` poles of `order` (at least `order` poles); open curves are
/// clamped, with the stored interior knots when there are `poles - order` of them in ascending
/// order, uniform ones otherwise. Periodic curves are uniform, for the poles with their first
/// `order - 1` repeated.
pub(crate) fn knot_vector(poles: usize, order: usize, interior: Option<Vec<f64>>, periodic: bool) -> Vec<f64> {
    if periodic {
        return (0..poles + order - 1 + order).map(|k| k as f64).collect();
    }
    let inner = interior.filter(|v| v.len() == poles - order && v.windows(2).all(|w| w[0] <= w[1]))
        .unwrap_or_else(|| (1..=poles - order).map(|k| k as f64 / (poles - order + 1) as f64).collect());
    std::iter::repeat_n(0.0, order).chain(inner).chain(std::iter::repeat_n(1.0, order)).collect()
}

pub(crate) fn point(v: glam::DVec3) -> dgn_model::Point { v.to_array() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knot_vectors_are_clamped_or_uniform() {
        assert_eq!(knot_vector(4, 4, None, false), [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(knot_vector(5, 3, Some(vec![0.25, 0.5]), false), [0.0, 0.0, 0.0, 0.25, 0.5, 1.0, 1.0, 1.0]);
        assert_eq!(knot_vector(5, 3, Some(vec![0.5, 0.25]), false), [0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0], "unsorted: uniform");
        assert_eq!(knot_vector(3, 2, None, true).len(), 3 + 2 * 2 - 1);
    }

    #[test]
    fn quaternions_are_stored_inverted() {
        // +30 degrees about Z is stored as (cos 15, 0, 0, -sin 15).
        let half = 15f64.to_radians();
        let m = quaternion(half.cos(), 0.0, 0.0, -half.sin());
        assert!((m.x_axis.y - 0.5).abs() < 1e-12 && (m.x_axis.x - 30f64.to_radians().cos()).abs() < 1e-12);
    }
}
