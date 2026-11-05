//! Writers of synthetic design files, built from the element layouts of the clean-room
//! specification (`SPEC_v0.1.md`), ISFF and [MS-CFB], for tests here and in dependent crates.
//! No sample files are involved. Enabled with the `synth` feature.

use crate::cfb::END_OF_CHAIN;
use crate::OLE_SIGNATURE;

/// zlib (RFC 1950) data of `data` at compression `level` (0 = stored).
pub fn zlib(data: &[u8], level: u32) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Writes a compound file (version 3, 512-byte sectors) holding `streams` by path
/// (`"Storage/Sub/Stream"`). Streams under 4096 bytes go into the mini stream; the FAT sectors
/// come last.
pub fn compound(streams: &[(String, Vec<u8>)]) -> Vec<u8> {
    const FREE: u32 = 0xFFFF_FFFF;
    struct Node { name: String, kind: u8, children: Vec<usize>, data: Vec<u8>, start: u32 }
    /// Appends `bytes` as a chain of `size`-byte sectors.
    fn chain_of(table: &mut Vec<u32>, sectors: &mut Vec<u8>, bytes: &[u8], size: usize) -> u32 {
        if bytes.is_empty() { return END_OF_CHAIN; }
        let first = table.len() as u32;
        let count = bytes.len().div_ceil(size);
        for k in 0..count { table.push(if k + 1 == count { END_OF_CHAIN } else { first + k as u32 + 1 }); }
        sectors.extend_from_slice(bytes);
        sectors.resize(table.len() * size, 0);
        first
    }
    /// Siblings as a binary tree through left and right; returns the middle one.
    fn tree(list: &[usize], links: &mut [(u32, u32)]) -> u32 {
        if list.is_empty() { return FREE; }
        let middle = list.len() / 2;
        links[list[middle]] = (tree(&list[..middle], links), tree(&list[middle + 1..], links));
        list[middle] as u32
    }
    let mut nodes = vec![Node { name: "Root Entry".into(), kind: 5, children: Vec::new(), data: Vec::new(), start: END_OF_CHAIN }];
    for (path, data) in streams {
        let parts: Vec<&str> = path.split('/').collect();
        let mut parent = 0;
        for (k, part) in parts.iter().enumerate() {
            let stream = k + 1 == parts.len();
            parent = match nodes[parent].children.iter().copied().find(|&c| nodes[c].name == *part) {
                Some(child) => child,
                None => {
                    let data = if stream { data.clone() } else { Vec::new() };
                    nodes.push(Node { name: part.to_string(), kind: if stream { 2 } else { 1 }, children: Vec::new(), data, start: END_OF_CHAIN });
                    let child = nodes.len() - 1;
                    nodes[parent].children.push(child);
                    child
                }
            };
        }
    }
    let (mut mini, mut mini_fat, mut sectors, mut fat) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for node in nodes.iter_mut().filter(|n| n.kind == 2) {
        node.start = if node.data.len() < 4096 { chain_of(&mut mini_fat, &mut mini, &node.data, 64) } else { chain_of(&mut fat, &mut sectors, &node.data, 512) };
    }
    let mini_fat_bytes: Vec<u8> = mini_fat.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mini_fat_start = chain_of(&mut fat, &mut sectors, &mini_fat_bytes, 512);
    nodes[0].start = chain_of(&mut fat, &mut sectors, &mini, 512);
    let mut links = vec![(FREE, FREE); nodes.len()];
    let children: Vec<u32> = nodes.iter().map(|n| tree(&n.children, &mut links)).collect();
    let mut directory = Vec::new();
    for (k, node) in nodes.iter().enumerate() {
        let mut entry = [0u8; 128];
        let name: Vec<u16> = node.name.encode_utf16().collect();
        for (j, c) in name.iter().enumerate() { entry[2 * j..2 * j + 2].copy_from_slice(&c.to_le_bytes()); }
        entry[0x40..0x42].copy_from_slice(&(2 * (name.len() as u16 + 1)).to_le_bytes());
        entry[0x42] = node.kind;
        entry[0x43] = 1;
        for (at, value) in [(0x44, links[k].0), (0x48, links[k].1), (0x4C, children[k]), (0x74, node.start)] {
            entry[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let size = if node.kind == 5 { mini.len() } else { node.data.len() };
        entry[0x78..0x80].copy_from_slice(&(size as u64).to_le_bytes());
        directory.extend(entry);
    }
    let directory_start = chain_of(&mut fat, &mut sectors, &directory, 512);
    // The FAT describes all sectors, its own included.
    let mut fat_count = 1;
    while fat_count * 128 < fat.len() + fat_count { fat_count += 1; }
    assert!(fat_count <= 109);
    let first_fat = fat.len() as u32;
    fat.extend(std::iter::repeat_n(0xFFFF_FFFD, fat_count));
    fat.resize(fat_count * 128, FREE);
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(&OLE_SIGNATURE);
    for (at, value) in [(0x18, 0x3Eu16), (0x1A, 3), (0x1C, 0xFFFE), (0x1E, 9), (0x20, 6)] { out[at..at + 2].copy_from_slice(&value.to_le_bytes()); }
    let header = [
        (0x2C, fat_count as u32), (0x30, directory_start), (0x38, 4096), (0x3C, mini_fat_start),
        (0x40, mini_fat_bytes.len().div_ceil(512) as u32), (0x44, END_OF_CHAIN), (0x48, 0),
    ];
    for (at, value) in header { out[at..at + 4].copy_from_slice(&value.to_le_bytes()); }
    for k in 0..109 {
        let value = if k < fat_count { first_fat + k as u32 } else { FREE };
        out[0x4C + 4 * k..0x50 + 4 * k].copy_from_slice(&value.to_le_bytes());
    }
    out.extend(sectors);
    out.extend(fat.iter().flat_map(|s| s.to_le_bytes()));
    out
}

/// V7 design files from the ISFF element layouts (words, longs high word first, VAX D-float).
pub mod v7 {
    /// A 32-bit integer as two words, high word first.
    pub fn long(v: i32) -> [u16; 2] { [((v as u32) >> 16) as u16, v as u32 as u16] }

    /// A double as VAX D-float, most significant word first.
    pub fn dfloat(v: f64) -> [u16; 4] {
        if v == 0.0 { return [0; 4]; }
        let bits = v.to_bits();
        let exponent = ((bits >> 52) & 0x7FF) - 894;
        let u = (bits & (1 << 63)) | (exponent << 55) | ((bits & ((1 << 52) - 1)) << 3);
        [(u >> 48) as u16, (u >> 32) as u16, (u >> 16) as u16, u as u16]
    }

    /// Writes a design file element by element.
    pub struct Writer {
        pub words: Vec<u16>,
        pub three_d: bool,
    }

    impl Writer {
        /// A design file header: `master` and `sub` unit names, subunits per master, UOR per
        /// subunit, the global origin in UOR.
        pub fn new(three_d: bool, master: &str, sub: &str, sub_per_master: i32, uor_per_sub: i32, origin: [f64; 3]) -> Self {
            let mut tcb = vec![0u16; 768];
            tcb[0] = 0x0908 | if three_d { 0xC0 } else { 0 };
            tcb[1] = 766;
            tcb[556..558].copy_from_slice(&long(sub_per_master));
            tcb[558..560].copy_from_slice(&long(uor_per_sub));
            let name = |s: &str| { let b = s.as_bytes(); u16::from(*b.first().unwrap_or(&0)) | (u16::from(*b.get(1).unwrap_or(&0)) << 8) };
            tcb[560] = name(master);
            tcb[561] = name(sub);
            for (k, &v) in origin.iter().enumerate() { tcb[620 + 4 * k..624 + 4 * k].copy_from_slice(&dfloat(v)); }
            Writer { words: tcb, three_d }
        }

        /// An element: header (range left zero), display header (`flags`: properties; 0x80 sets
        /// the complex bit), then `body` from word 18.
        pub fn element(&mut self, kind: u8, level: u8, color: u8, weight: u8, flags: u16, body: &[u16]) -> &mut Self {
            let complex = flags & 0x80;
            let mut e = vec![u16::from(kind) << 8 | u16::from(level) | complex, 0];
            e.extend([0u16; 12]);
            e.extend([0, 0, flags & 0xFF0F, u16::from(color) << 8 | u16::from(weight) << 3]);
            e.extend_from_slice(body);
            e[1] = (e.len() - 2) as u16;
            e[15] = (e.len() - 16) as u16;
            self.words.extend(e);
            self
        }

        /// Points of longs (two or three coordinates by the file's dimension).
        pub fn points(&self, points: &[[i32; 3]]) -> Vec<u16> {
            points.iter().flat_map(|p| p[..if self.three_d { 3 } else { 2 }].iter().flat_map(|&v| long(v)).collect::<Vec<_>>()).collect()
        }

        /// The file, ended by the end-of-design word.
        pub fn bytes(&self) -> Vec<u8> {
            self.words.iter().chain([0xFFFF].iter()).flat_map(|w| w.to_le_bytes()).collect()
        }
    }
}

/// V8 design files: elements (B.3, B.4, B.7) in zlib-compressed element streams (B.2) of a
/// compound file (B.1), with a model header (B.5).
pub mod v8 {
    use super::{compound, zlib};

    /// Properties bit: a 3D element.
    pub const D3: u32 = 0x0800;
    /// Properties bit: a hole (H).
    pub const HOLE: u32 = 0x8000;

    /// A V8 element (B.3): the header, a display header for graphic elements (`display`:
    /// properties, colour, weight), the data from +0x68 (or +0x20), then string linkages (B.7).
    pub fn element(kind: u16, flags: u16, level: u32, display: Option<[u32; 3]>, data: &[u8], strings: &[(u32, &str)]) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend(kind.to_le_bytes());
        e.extend(flags.to_le_bytes());
        e.extend([0; 8]);
        e.extend(level.to_le_bytes());
        e.extend(0x1234u64.to_le_bytes());
        e.extend(0f64.to_le_bytes());
        if let Some([properties, color, weight]) = display {
            for value in [0, 0x8000_0000, properties, 0, weight, color] { e.extend(value.to_le_bytes()); }
            e.extend([0; 48]);
        }
        e.extend_from_slice(data);
        if e.len() % 2 == 1 { e.push(0); }
        let attindx = (e.len() / 2) as u32;
        e[8..12].copy_from_slice(&attindx.to_le_bytes());
        with_strings(e, strings)
    }

    /// Appends string linkages (§B.7): header (user bit, the number of words after it), ID
    /// 0x56D2, key, number of bytes, the bytes; and updates the element's length.
    pub fn with_strings(mut e: Vec<u8>, strings: &[(u32, &str)]) -> Vec<u8> {
        for (key, text) in strings {
            let mut linkage = vec![0, 0, 0xD2, 0x56];
            linkage.extend(key.to_le_bytes());
            linkage.extend((text.len() as u32).to_le_bytes());
            linkage.extend(text.as_bytes());
            if linkage.len() % 2 == 1 { linkage.push(0); }
            let words = (linkage.len() / 2) as u16;
            linkage[..2].copy_from_slice(&(0x1000 | (words - 1)).to_le_bytes());
            e.extend(linkage);
        }
        let words = (e.len() / 2) as u32;
        e[4..8].copy_from_slice(&words.to_le_bytes());
        e
    }

    /// A model header stream (§B.5, §B.15), compressed: `uor` UOR per storage unit,
    /// `numerator / denominator` storage units per metre, the global origin in UOR, the master
    /// unit the metre (named `m`), the sub unit the millimetre (`mm`), the model's name and 3D bit.
    pub fn model_header_stream(uor: f64, numerator: f64, denominator: f64, origin: [f64; 3], name: Option<&str>, three_d: bool) -> Vec<u8> {
        model_header_stream_with(uor, numerator, denominator, origin, name, three_d, false)
    }

    /// [`model_header_stream`] of a sheet model (+0x22 = 1) when `sheet`.
    pub fn model_header_stream_with(uor: f64, numerator: f64, denominator: f64, origin: [f64; 3], name: Option<&str>, three_d: bool, sheet: bool) -> Vec<u8> {
        let mut e = vec![0u8; 0x100];
        e[0x22..0x24].copy_from_slice(&u16::from(sheet).to_le_bytes());
        e[..2].copy_from_slice(&66u16.to_le_bytes());
        e[2..4].copy_from_slice(&0x0400u16.to_le_bytes());
        e[8..12].copy_from_slice(&0x80u32.to_le_bytes());
        e[0x0C..0x10].copy_from_slice(&32u32.to_le_bytes());
        e[0x44..0x48].copy_from_slice(&u32::from(three_d).to_le_bytes());
        let fields = [
            (0x50, 1.0), (0x58, 1.0), (0x60, 1000.0), (0x68, 1.0), (0x70, origin[0]), (0x78, origin[1]), (0x80, origin[2]),
            (0xE0, uor), (0xE8, numerator), (0xF0, denominator),
        ];
        for (at, value) in fields { e[at..at + 8].copy_from_slice(&f64::to_le_bytes(value)); }
        let mut strings = vec![(19, "m"), (20, "mm")];
        if let Some(name) = name { strings.push((1, name)); }
        let mut plain = vec![0u8; 4096 + 4];
        plain.extend(with_strings(e, &strings));
        zlib(&plain, 6)
    }

    /// The file header stream `Dgn~H` (§B.14): 20 bytes, then zlib data of 1576 bytes with the
    /// format revision at +0x118.
    pub fn file_header(revision: u32) -> Vec<u8> {
        let mut plain = vec![0u8; 1576];
        for (at, value) in [(0x114, 8u32), (0x118, revision), (0x11C, 2)] { plain[at..at + 4].copy_from_slice(&value.to_le_bytes()); }
        let mut out = vec![1, 0, 4, 0];
        out.resize(20, 0);
        out.extend(zlib(&plain, 6));
        out
    }

    /// An OLE summary information stream ([MS-OLEPS]) with the application name (property 18).
    pub fn summary_information(application: &str) -> Vec<u8> {
        let mut text = application.as_bytes().to_vec();
        text.push(0);
        let mut value = 0x1Eu32.to_le_bytes().to_vec();
        value.extend((text.len() as u32).to_le_bytes());
        value.extend(&text);
        value.resize(value.len().div_ceil(4) * 4, 0);
        // The set: size, one property (18 at offset 16), the value.
        let mut set = ((16 + value.len()) as u32).to_le_bytes().to_vec();
        set.extend(1u32.to_le_bytes());
        set.extend(18u32.to_le_bytes());
        set.extend(16u32.to_le_bytes());
        set.extend(value);
        let mut out = vec![0xFE, 0xFF, 0, 0];
        out.extend([0; 4 + 16]);
        out.extend(1u32.to_le_bytes());
        out.extend([0; 16]);
        out.extend(48u32.to_le_bytes());
        out.extend(set);
        out
    }

    /// A cone (§B.16) between circles of `r1` around `c1` and `r2` around `c2`, with the identity
    /// orientation.
    pub fn cone(level: u32, c1: [f64; 3], r1: f64, c2: [f64; 3], r2: f64) -> Vec<u8> {
        let mut data = vec![0u8; 8];
        data.extend(doubles(&[1.0, 0.0, 0.0, 0.0]));
        data.extend(doubles(&c1));
        data.extend(doubles(&[r1]));
        data.extend(doubles(&c2));
        data.extend(doubles(&[r2]));
        element(23, 0x1000, level, shown(D3, 0), &data, &[])
    }

    /// A mesh (§B.17): the header 105, then a vertex array (101 role 2, 103 of doubles) and an
    /// index array (101 role 1, 102 of i32: 1-based, 0 closes a face).
    pub fn mesh(level: u32, vertices: &[[f64; 3]], indices: &[i32]) -> Vec<Vec<u8>> {
        let header = |role: u32, per_item: u32| {
            let mut data = 0u32.to_le_bytes().to_vec();
            for value in [per_item, 1, role] { data.extend(value.to_le_bytes()); }
            element(101, 0x4000, level, shown(D3, 0), &data, &[])
        };
        let array = |kind: u16, count: usize, values: Vec<u8>| {
            let mut data = Vec::new();
            for value in [count as u32, count as u32, 0x1121, 0] { data.extend(value.to_le_bytes()); }
            data.extend(values);
            element(kind, 0x4000, level, shown(D3, 0), &data, &[])
        };
        let mut head = 4u32.to_le_bytes().to_vec();
        head.extend(1u32.to_le_bytes());
        vec![
            element(105, 0x2000, level, shown(D3, 0), &head, &[]),
            header(2, 3),
            array(103, 3 * vertices.len(), doubles(&vertices.concat())),
            header(1, 1),
            array(102, indices.len(), indices.iter().flat_map(|i| i.to_le_bytes()).collect()),
        ]
    }

    /// An attachment (type 100, §B.20) of the model `model` of the same file: no file name, the
    /// logical name in string linkage 4 and the model's name in 21, placed at `at` (UOR) with
    /// identity matrix and the stored `scale`.
    pub fn model_attachment(number: u32, model: &str, logical: &str, at: [f64; 3], scale: f64) -> Vec<u8> {
        let mut data = vec![0u8; 0xA8 - 0x68];
        data.extend(doubles(&[0.0; 3]));
        data.extend(doubles(&at));
        data.extend(doubles(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, scale]));
        let mut e = element(100, 0x2040, 0, Some([0, 0, 0]), &data, &[(4, logical), (21, model)]);
        e[0x2C..0x30].copy_from_slice(&number.to_le_bytes());
        e
    }

    /// A raster (§B.21) in format 2 with transparent background 0: `rows` of colour indices from
    /// the top, `pixel` UOR per pixel, the upper left corner at `origin`. The header and one scan
    /// line element per row.
    pub fn raster(level: u32, rows: &[&[u8]], pixel: f64, origin: [f64; 3]) -> Vec<Vec<u8>> {
        let flags: u16 = (2 << 3) | 0x4 | 0x200;
        let mut head = (rows.len() as u32).to_le_bytes().to_vec();
        head.extend(flags.to_le_bytes());
        head.extend([7, 0]);
        for value in [256u16, rows[0].len() as u16, rows.len() as u16] { head.extend(value.to_le_bytes()); }
        head.resize(0x80 - 0x68, 0);
        head.extend(doubles(&[pixel]));
        head.extend(doubles(&origin));
        let mut out = vec![element(87, 0x3000, level, shown(0, 0), &head, &[])];
        for (y, row) in rows.iter().enumerate() {
            let mut data = flags.to_le_bytes().to_vec();
            data.extend([7, 0]);
            data.extend(256u32.to_le_bytes());
            data.extend((y as u16).to_le_bytes());
            data.extend((row.len() as u16).to_le_bytes());
            data.extend(*row);
            out.push(element(88, 0x4000, level, shown(0, 0), &data, &[]));
        }
        out
    }

    /// A reference attachment (type 100, §B.20) with sequence `number`, the file name in string
    /// linkage 3, the master-side origin `at` (UOR), identity matrix and scale 1.
    pub fn reference_attachment(number: u32, file: &str, at: [f64; 3]) -> Vec<u8> {
        let mut data = vec![0u8; 0xA8 - 0x68];
        data.extend(doubles(&[0.0; 3]));
        data.extend(doubles(&at));
        data.extend(doubles(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0]));
        let mut e = element(100, 0x2040, 0, Some([0, 0, 0]), &data, &[(3, file)]);
        e[0x2C..0x30].copy_from_slice(&number.to_le_bytes());
        e
    }

    /// A display header with `properties` and `color`, weight 0.
    pub fn shown(properties: u32, color: u32) -> Option<[u32; 3]> { Some([properties, color, 0]) }

    pub fn doubles(values: &[f64]) -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() }

    /// A count, four bytes of padding and the coordinates of the points.
    pub fn counted(count: u32, coordinates: &[f64]) -> Vec<u8> {
        let mut data = count.to_le_bytes().to_vec();
        data.extend([0; 4]);
        data.extend(doubles(coordinates));
        data
    }

    /// A level table entry (B.6) with its name.
    pub fn level_entry(id: u32, name: &str) -> Vec<u8> {
        let mut data = id.to_le_bytes().to_vec();
        data.extend((-1i32).to_le_bytes());
        element(95, 0x0400, 1, None, &data, &[(1, name)])
    }

    /// A level entry with its line style, weight and colour at +0x40 (B.13); the override
    /// symbology at +0x34 holds other values, which must not be used.
    pub fn level_with_symbology(id: u32, name: &str, style: i32, weight: u32, color: u32) -> Vec<u8> {
        let mut data = id.to_le_bytes().to_vec();
        data.extend((-1i32).to_le_bytes());
        data.extend([0; 0x0C]);
        for value in [1u32, 31, 3, style as u32, weight, color, color] { data.extend(value.to_le_bytes()); }
        element(95, 0x0400, 1, None, &data, &[(1, name)])
    }

    /// A colour table (B.6): RGB `slots` from +0x22.
    pub fn color_entry(slots: &[(usize, [u8; 3])]) -> Vec<u8> {
        let mut data = vec![0u8; 2 + 768];
        for &(slot, rgb) in slots { data[2 + 3 * slot..5 + 3 * slot].copy_from_slice(&rgb); }
        element(5, 0x0400, 1, None, &data, &[])
    }

    /// Text data (B.4) from +0x68: 2D, at `origin`, `size` (width, height) in UOR, with `bytes` as
    /// its characters. The multipliers give characters of 6 UOR, so a box from them would be tiny.
    pub fn text_data(size: [f64; 2], origin: [f64; 2], bytes: &[u8]) -> Vec<u8> {
        let mut data = 0u32.to_le_bytes().to_vec();
        data.extend(0u16.to_le_bytes());
        data.extend([bytes.len() as u8, 0]);
        data.extend(doubles(&[1000.0, 1000.0, size[0], size[1], 0.0, origin[0], origin[1]]));
        data.extend(0u16.to_le_bytes());
        data.extend(bytes);
        data
    }

    /// The header of a shared cell or its definition (B.4): range, row-major matrix, origin.
    pub fn shared_data(matrix: [f64; 9], origin: [f64; 3]) -> Vec<u8> {
        let mut data = vec![0u8; 8];
        data.extend(doubles(&[0.0; 6]));
        data.extend(doubles(&matrix));
        data.extend(doubles(&origin));
        data
    }

    /// A 2D dimension (B.10) on `level`: `points` (x, y), the height `h` of the first point, the
    /// 2 x 2 matrix `m` and the element range (`low`, `size`, UOR).
    pub fn dimension(level: u32, points: &[[f64; 2]], h: f64, m: [f64; 4], low: [i64; 2], size: [i64; 2], properties: u32) -> Vec<u8> {
        let mut data = vec![0u8; 0x130 - 0x68 + 0x30 * points.len()];
        data[0x87 - 0x68] = points.len() as u8;
        data[0x110 - 0x68..0x130 - 0x68].copy_from_slice(&doubles(&m));
        for (i, p) in points.iter().enumerate() {
            let at = 0x130 - 0x68 + 0x30 * i;
            data[at..at + 0x20].copy_from_slice(&doubles(&[p[0], p[1], 0.0, if i == 0 { h } else { 0.0 }]));
        }
        let mut e = element(33, 0x1000, level, shown(properties, 0), &data, &[]);
        set_range(&mut e, [low[0], low[1], 0], [size[0], size[1], 0]);
        e
    }

    /// Sets the stored range of an element: low corner and size (B.3).
    pub fn set_range(element: &mut [u8], low: [i64; 3], size: [i64; 3]) {
        for (k, value) in low.iter().chain(&size).enumerate() {
            element[0x38 + 8 * k..0x40 + 8 * k].copy_from_slice(&value.to_le_bytes());
        }
    }

    /// Adds a fill linkage (B.7): header `07 10`, ID 0x0041, 8 at +4, the fill colour at +8, 0.
    pub fn with_fill(mut element: Vec<u8>, color: u32) -> Vec<u8> {
        element.extend([0x07, 0x10, 0x41, 0x00]);
        for value in [8u32, color, 0] { element.extend(value.to_le_bytes()); }
        let words = (element.len() / 2) as u32;
        element[4..8].copy_from_slice(&words.to_le_bytes());
        element
    }

    /// A further model: its ID, whether it is hidden (`Dgn-Nd`), header and graphic elements.
    pub struct ExtraModel {
        pub id: u32,
        pub hidden: bool,
        pub header: Vec<u8>,
        pub graphics: Vec<u8>,
        pub controls: Vec<u8>,
    }

    /// A V8 design file: the default model (header, graphic and control elements), the
    /// non-model elements, further models, file header and summary information, and any further
    /// streams.
    pub struct File {
        pub header: Vec<u8>,
        pub graphics: Vec<u8>,
        pub controls: Vec<u8>,
        pub tables: Vec<u8>,
        pub models: Vec<ExtraModel>,
        pub revision: Option<u32>,
        pub application: Option<String>,
        pub extra: Vec<(String, Vec<u8>)>,
    }

    /// Appends an element with its 4-byte prefix to an element stream.
    pub fn push(stream: &mut Vec<u8>, element: Vec<u8>) {
        stream.extend([0; 4]);
        stream.extend(element);
    }

    impl File {
        /// `uor` UOR per storage unit, `numerator / denominator` storage units per metre, the
        /// global origin in UOR (B.5). The master unit is the metre, the sub unit the millimetre;
        /// the default model is a 2D model without a name.
        pub fn new(uor: f64, numerator: f64, denominator: f64, origin: [f64; 3]) -> Self {
            File {
                header: model_header_stream(uor, numerator, denominator, origin, None, false),
                graphics: Vec::new(), controls: Vec::new(), tables: Vec::new(), models: Vec::new(), revision: None, application: None, extra: Vec::new(),
            }
        }

        /// Adds a graphic element of the default model.
        pub fn graphic(&mut self, element: Vec<u8>) -> &mut Self {
            push(&mut self.graphics, element);
            self
        }

        /// Adds a control element of the default model (`Dgn^C`).
        pub fn control(&mut self, element: Vec<u8>) -> &mut Self {
            push(&mut self.controls, element);
            self
        }

        /// Adds a non-model element (`Dgn^Nm`).
        pub fn table(&mut self, element: Vec<u8>) -> &mut Self {
            push(&mut self.tables, element);
            self
        }

        /// The compound file. The graphic elements are cut at arbitrary bytes into `$1` (stored,
        /// up to 5000 bytes, so it can outgrow the mini stream) and `$2` to `$11` (compressed):
        /// elements run across stream boundaries and `$10` must follow `$9`.
        pub fn bytes(&self) -> Vec<u8> {
            let block = |data: &[u8], level: u32| {
                let mut block: Vec<u8> = [1u32, 3, 1, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
                block.extend(zlib(data, level));
                block
            };
            let first = (self.graphics.len() / 2).min(5000);
            let rest = &self.graphics[first..];
            let size = rest.len().div_ceil(10);
            let mut streams = vec![("Dgn-Md/#000000/Dgn^G/$1".to_string(), block(&self.graphics[..first], 0))];
            for k in 0..10 {
                let part = &rest[(k * size).min(rest.len())..((k + 1) * size).min(rest.len())];
                streams.push((format!("Dgn-Md/#000000/Dgn^G/${}", k + 2), block(part, 6)));
            }
            streams.push(("Dgn^Nm/$1".into(), block(&self.tables, 6)));
            streams.push(("Dgn-Md/#000000/Dgn~Mh".into(), self.header.clone()));
            streams.push(("Dgn-Md/#000000/Dgn^C/$1".into(), if self.controls.is_empty() { vec![0; 16] } else { block(&self.controls, 6) }));
            for m in &self.models {
                let storage = format!("{}/#{:06x}", if m.hidden { "Dgn-Nd" } else { "Dgn-Md" }, m.id);
                streams.push((format!("{storage}/Dgn~Mh"), m.header.clone()));
                streams.push((format!("{storage}/Dgn^G/$1"), block(&m.graphics, 6)));
                if !m.controls.is_empty() { streams.push((format!("{storage}/Dgn^C/$1"), block(&m.controls, 6))); }
            }
            streams.push(("Dgn~H".into(), self.revision.map_or(vec![0; 80], file_header)));
            if let Some(application) = &self.application { streams.push(("\u{5}SummaryInformation".into(), summary_information(application))); }
            streams.extend(self.extra.iter().cloned());
            compound(&streams)
        }
    }
}
