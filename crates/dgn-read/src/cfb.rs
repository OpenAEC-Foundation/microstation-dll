//! The OLE2 compound file that holds a V8 design file, read from memory after Microsoft's public
//! [MS-CFB] specification: header, FAT (with DIFAT chain), directory, mini FAT and mini stream.
//!
//! Every chain is followed at most once per sector and never beyond the file, so a crafted file
//! cannot make the reader loop or multiply its tables.

use crate::{le16, le32, le64, Error, Result, OLE_SIGNATURE};

/// Sector numbers above this are markers: free, end of chain, FAT or DIFAT sector.
const MAX_SECTOR: u32 = 0xFFFF_FFFA;
pub(crate) const END_OF_CHAIN: u32 = 0xFFFF_FFFE;

/// The kind of a directory entry.
pub const STORAGE: u8 = 1;
pub const STREAM: u8 = 2;
pub const ROOT: u8 = 5;

/// A directory entry: a storage, stream or the root.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// [`STORAGE`], [`STREAM`] or [`ROOT`].
    pub kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    pub size: u64,
}

/// A compound file read from memory: its allocation tables, directory and mini stream.
pub struct CompoundFile<'a> {
    data: &'a [u8],
    shift: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    /// Streams smaller than this live in the mini stream.
    cutoff: u64,
    pub entries: Vec<Entry>,
}

/// The sectors of the chain that starts at `start`, at most `limit` of them; a chain that comes
/// back to a sector ends there.
pub(crate) fn chain(table: &[u32], start: u32, limit: usize) -> Vec<u32> {
    let mut seen = vec![false; table.len()];
    let mut out = Vec::new();
    let mut sector = start;
    while sector <= MAX_SECTOR && out.len() < limit {
        let index = sector as usize;
        if let Some(seen) = seen.get_mut(index) {
            if std::mem::replace(seen, true) { break; }
        }
        out.push(sector);
        sector = table.get(index).copied().unwrap_or(END_OF_CHAIN);
    }
    out
}

fn u32s(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
}

impl<'a> CompoundFile<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self> {
        let invalid = |what: &str| Error(format!("MicroStation DGN: not a readable V8 design file ({what})"));
        if data.len() < 512 || !data.starts_with(&OLE_SIGNATURE) { return Err(invalid("too short for a compound file")); }
        let shift = usize::from(le16(data, 0x1E));
        if shift != 9 && shift != 12 { return Err(invalid("unknown sector size")); }
        if le16(data, 0x20) != 6 { return Err(invalid("unknown mini sector size")); }
        let mut file = CompoundFile {
            data, shift, fat: Vec::new(), mini_fat: Vec::new(), mini_stream: Vec::new(), cutoff: u64::from(le32(data, 0x38)), entries: Vec::new(),
        };
        // The FAT sectors: 109 in the header, the rest in a chain of DIFAT sectors whose last
        // number points to the next one. A file has no more FAT sectors than sectors, and each
        // DIFAT sector is read once, so a crafted file cannot multiply the table.
        let sectors = data.len() >> shift;
        let mut fat_sectors: Vec<u32> = u32s(&data[0x4C..0x200]).filter(|&s| s <= MAX_SECTOR).collect();
        let mut difat_seen = vec![false; sectors + 1];
        let mut next = le32(data, 0x44);
        while next <= MAX_SECTOR && fat_sectors.len() < sectors {
            if difat_seen.get_mut(next as usize).is_none_or(|seen| std::mem::replace(seen, true)) { break; }
            let Some(sector) = file.sector(next) else { break };
            let per = sector.len() / 4 - 1;
            fat_sectors.extend(u32s(&sector[..4 * per]).filter(|&s| s <= MAX_SECTOR));
            next = le32(sector, 4 * per);
        }
        fat_sectors.truncate(sectors);
        let fat: Vec<u32> = fat_sectors.into_iter().filter_map(|s| file.sector(s)).flat_map(u32s).collect();
        file.fat = fat;
        let directory = file.regular(le32(data, 0x30), None);
        let version = le16(data, 0x1A);
        file.entries = directory.chunks_exact(128).map(|d| {
            let chars = (usize::from(le16(d, 0x40)) / 2).saturating_sub(1).min(31);
            Entry {
                name: String::from_utf16_lossy(&(0..chars).map(|k| le16(d, 2 * k)).collect::<Vec<_>>()),
                kind: d[0x42], left: le32(d, 0x44), right: le32(d, 0x48), child: le32(d, 0x4C), start: le32(d, 0x74),
                // Version 3 files may leave garbage in the high half of the size.
                size: if version == 3 { u64::from(le32(d, 0x78)) } else { le64(d, 0x78) },
            }
        }).collect();
        let Some(root) = file.entries.first().filter(|root| root.kind == ROOT) else { return Err(invalid("no root directory entry")) };
        let (root_start, root_size) = (root.start, root.size);
        file.mini_fat = u32s(&file.regular(le32(data, 0x3C), None)).collect();
        file.mini_stream = file.regular(root_start, Some(root_size));
        Ok(file)
    }

    /// Sector `n` (the last one of a truncated file may be short).
    fn sector(&self, n: u32) -> Option<&'a [u8]> {
        let start = (n as usize).checked_add(1)?.checked_mul(1 << self.shift)?;
        let data: &'a [u8] = self.data;
        data.get(start..data.len().min(start + (1 << self.shift))).filter(|s| s.len() >= 4)
    }

    /// The bytes of a chain of sectors, at most `size` and never more than the file.
    fn regular(&self, start: u32, size: Option<u64>) -> Vec<u8> {
        let limit = size.map_or(usize::MAX, |s| usize::try_from(s).unwrap_or(usize::MAX)).min(self.data.len());
        let mut out = Vec::new();
        for sector in chain(&self.fat, start, (self.data.len() >> self.shift) + 1) {
            if out.len() >= limit { break; }
            match self.sector(sector) { Some(bytes) => out.extend_from_slice(bytes), None => break }
        }
        out.truncate(limit);
        out
    }

    /// The bytes of a chain of 64-byte sectors in the mini stream.
    fn mini(&self, start: u32, size: u64) -> Vec<u8> {
        let limit = usize::try_from(size).unwrap_or(usize::MAX).min(self.mini_stream.len());
        let mut out = Vec::new();
        for sector in chain(&self.mini_fat, start, self.mini_stream.len().div_ceil(64)) {
            let at = sector as usize * 64;
            match self.mini_stream.get(at..self.mini_stream.len().min(at + 64)) {
                Some(bytes) if !bytes.is_empty() && out.len() < limit => out.extend_from_slice(bytes),
                _ => break,
            }
        }
        out.truncate(limit);
        out
    }

    /// The contents of stream `entry`.
    pub fn read(&self, entry: usize) -> Vec<u8> {
        let e = &self.entries[entry];
        if e.size < self.cutoff { self.mini(e.start, e.size) } else { self.regular(e.start, Some(e.size)) }
    }

    /// The entries of storage `parent`: a tree through the left and right siblings of its child.
    pub fn children(&self, parent: usize) -> Vec<usize> {
        let mut seen = vec![false; self.entries.len()];
        let mut stack = vec![self.entries[parent].child];
        let mut out = Vec::new();
        while let Some(k) = stack.pop() {
            let k = k as usize;
            let Some(e) = self.entries.get(k) else { continue };
            if std::mem::replace(&mut seen[k], true) { continue; }
            out.push(k);
            stack.extend([e.right, e.left]);
        }
        out
    }

    /// The child of storage `parent` called `name` (case-sensitive).
    pub fn child(&self, parent: usize, name: &str) -> Option<usize> {
        self.children(parent).into_iter().find(|&k| self.entries[k].name == name)
    }

    /// The entry at a path of names below the root.
    pub fn path(&self, names: &[&str]) -> Option<usize> {
        names.iter().try_fold(0, |at, name| self.child(at, name))
    }

    /// Every entry below the root as `(path, entry)`, storages before their contents.
    pub fn tree(&self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.entries.len()];
        let mut stack = vec![(String::new(), 0usize)];
        while let Some((prefix, parent)) = stack.pop() {
            let mut children = self.children(parent);
            children.sort_by(|&a, &b| self.entries[a].name.cmp(&self.entries[b].name));
            for k in children.into_iter().rev() {
                if std::mem::replace(&mut seen[k], true) { continue; }
                let path = if prefix.is_empty() { self.entries[k].name.clone() } else { format!("{prefix}/{}", self.entries[k].name) };
                if self.entries[k].kind == STORAGE { stack.push((path.clone(), k)); }
                out.push((path, k));
            }
        }
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::compound;

    #[test]
    fn crafted_compound_files_cannot_multiply_their_chains() {
        // 20 KB with 4 KB sectors; the FAT list in the header, the DIFAT chain, the directory and
        // the mini FAT all point at sector 0, which is zeros: every chain is a cycle on one sector.
        let mut data = vec![0u8; 20 * 1024];
        data[..8].copy_from_slice(&OLE_SIGNATURE);
        for (at, value) in [(0x18, 0x3Eu16), (0x1A, 4), (0x1C, 0xFFFE), (0x1E, 12), (0x20, 6)] { data[at..at + 2].copy_from_slice(&value.to_le_bytes()); }
        for (at, value) in [(0x2C, 1000u32), (0x38, 4096), (0x40, 1000), (0x48, 1000)] { data[at..at + 4].copy_from_slice(&value.to_le_bytes()); }
        assert!(CompoundFile::open(&data).err().unwrap().0.contains("no root directory entry"));
        // The same with a directory in sector 1: a root whose mini stream claims 2^48 bytes.
        data[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
        let root = &mut data[2 * 4096..3 * 4096];
        root[0x40] = 2;
        root[0x42] = 5;
        root[0x4C..0x50].copy_from_slice(&u32::MAX.to_le_bytes());
        root[0x78..0x80].copy_from_slice(&(1u64 << 48).to_le_bytes());
        let file = CompoundFile::open(&data).unwrap();
        assert!(file.fat.len() <= data.len() / 4, "{} FAT entries", file.fat.len());
        assert!(file.mini_stream.len() <= data.len() && file.mini_fat.len() <= data.len() / 4);
        assert!(file.path(&["Dgn-Md"]).is_none());
        assert_eq!(chain(&[0; 8], 0, 100), [0], "a chain ends where it comes back");
        assert_eq!(chain(&[1, 2, 3, 0], 0, 2), [0, 1], "and after its limit");
    }

    #[test]
    fn streams_in_the_mini_stream_and_in_sectors_read_back() {
        let small: Vec<u8> = (0..100u8).collect();
        let large: Vec<u8> = (0..10_000u32).map(|k| (k % 251) as u8).collect();
        let data = compound(&[("A/B/small".into(), small.clone()), ("A/large".into(), large.clone()), ("C".into(), vec![7; 4096])]);
        let file = CompoundFile::open(&data).unwrap();
        assert_eq!(file.read(file.path(&["A", "B", "small"]).unwrap()), small);
        assert_eq!(file.read(file.path(&["A", "large"]).unwrap()), large);
        assert_eq!(file.read(file.path(&["C"]).unwrap()), vec![7; 4096]);
        assert!(file.path(&["a"]).is_none(), "names are case-sensitive");
        let paths: Vec<String> = file.tree().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, ["A", "A/B", "A/B/small", "A/large", "C"]);
    }
}
