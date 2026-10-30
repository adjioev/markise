//! Length-prefixed frames: `u32 LE + JSON`, then `u32 LE + binary`.

#![allow(dead_code)]

use std::io::{self, Read, Write};

/// Largest accepted part (JSON or binary) of a frame.
pub const MAX_PART: usize = 256 << 20;

fn read_part(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_PART {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Reads one frame: (JSON, binary).
pub fn read_frame(r: &mut impl Read) -> io::Result<(serde_json::Value, Vec<u8>)> {
    let json = read_part(r)?;
    let bin = read_part(r)?;
    let v = serde_json::from_slice(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((v, bin))
}

/// Writes one frame and flushes.
pub fn write_frame(w: &mut impl Write, json: &serde_json::Value, bin: &[u8]) -> io::Result<()> {
    let j = serde_json::to_vec(json)?;
    w.write_all(&(j.len() as u32).to_le_bytes())?;
    w.write_all(&j)?;
    w.write_all(&(bin.len() as u32).to_le_bytes())?;
    w.write_all(bin)?;
    w.flush()
}
