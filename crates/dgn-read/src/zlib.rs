//! zlib (RFC 1950) inflation with a hard output limit.

use std::io::Read;

/// Inflates zlib data onto `out` until `out` holds `limit` bytes. Memory is reserved step by
/// step and never beyond `limit`, so a small compressed bomb or a full memory becomes an error
/// instead of an abort. The error says what was lost.
pub fn inflate(packed: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), &'static str> {
    let mut decoder = flate2::read::ZlibDecoder::new(packed);
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match decoder.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("Unreadable element stream (rest of the model)"),
        };
        let kept = read.min(limit.saturating_sub(out.len()));
        if out.capacity() - out.len() < kept {
            let capacity = (2 * out.capacity()).max(out.len() + kept).min(limit);
            out.try_reserve_exact(capacity - out.len()).map_err(|_| "Not enough memory for the element streams (rest of the model)")?;
        }
        out.extend_from_slice(&buffer[..kept]);
        if kept < read { return Err("Element streams too large (rest of the model)"); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_bombs_stop_at_the_limit() {
        let packed = crate::synth::zlib(&vec![0u8; 1 << 20], 9);
        let mut out = Vec::new();
        assert!(inflate(&packed, &mut out, 1000).is_err());
        assert_eq!((out.len(), out.capacity() <= 1000), (1000, true));
        let mut out = Vec::new();
        assert!(inflate(&packed, &mut out, 1 << 20).is_ok() && out.len() == 1 << 20);
        assert!(inflate(b"not zlib", &mut Vec::new(), 100).is_err());
    }
}
