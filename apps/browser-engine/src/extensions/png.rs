//! Minimal pure-Rust uncompressed RGBA -> PNG data URL encoder for `chrome.action.setIcon({ imageData })`.
//! Bounded to Chrome's toolbar icon limits (1..=128 px per side, exact `width * height * 4` bytes).

#![forbid(unsafe_code)]

use base64::Engine as _;

const MAX_SIDE: u32 = 128;

pub fn rgba_to_png_data_url(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if rgba.len() != expected {
        return None;
    }

    // Raw PNG scanlines: filter byte (0 = None) + width*4 RGBA bytes per row.
    let stride = (width as usize) * 4;
    let raw_len = (height as usize) * (1 + stride);
    let mut raw = Vec::with_capacity(raw_len);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * stride..(y + 1) * stride]);
    }

    // Wrap raw scanlines in a valid zlib stream using uncompressed DEFLATE stored blocks.
    let mut zlib = Vec::with_capacity(raw.len() + 32);
    zlib.push(0x78);
    zlib.push(0x01);
    let mut offset = 0;
    while offset < raw.len() {
        let take = (raw.len() - offset).min(65_535);
        let is_last = offset + take == raw.len();
        zlib.push(u8::from(is_last));
        let len = take as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + take]);
        offset += take;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(zlib.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // RGBA
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Some(format!("data:image/png;base64,{b64}"))
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let crc = crc32(tag, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(tag: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in tag.iter().chain(data.iter()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
