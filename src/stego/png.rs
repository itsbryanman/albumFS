use std::path::Path;

use image::RgbaImage;

use crate::stego::{
    decode_header, encode_header, CarrierCodec, ChunkMeta, CodecError, CHUNK_HEADER_LEN,
};

pub struct PngCodec;

impl PngCodec {
    fn raw_capacity(w: u32, h: u32) -> u64 {
        // 3 usable channels, 1 bit each, 8 bits per byte
        (w as u64 * h as u64 * 3) / 8
    }
}

impl CarrierCodec for PngCodec {
    fn capacity_bytes(&self, path: &Path) -> Result<u64, CodecError> {
        let (w, h) = image::image_dimensions(path)?;
        Ok(Self::raw_capacity(w, h).saturating_sub(CHUNK_HEADER_LEN as u64))
    }

    fn write_chunk(
        &self,
        path: &Path,
        meta: ChunkMeta,
        payload: &[u8],
        _order_key: &[u8],
    ) -> Result<(), CodecError> {
        let mut img = image::open(path)?.to_rgba8();
        let (w, h) = img.dimensions();
        let raw = Self::raw_capacity(w, h);
        let need = CHUNK_HEADER_LEN as u64 + payload.len() as u64;
        if need > raw {
            return Err(CodecError::PayloadTooLarge {
                payload: payload.len() as u64,
                capacity: raw - CHUNK_HEADER_LEN as u64,
            });
        }

        let header = encode_header(meta, payload.len() as u32);
        let mut bits = BitReader::new(&header, payload);

        'outer: for px in img.pixels_mut() {
            for ch in 0..3 {
                match bits.next() {
                    Some(bit) => px.0[ch] = (px.0[ch] & 0xFE) | bit,
                    None => break 'outer,
                }
            }
        }

        // Atomic replace: write temp in the same directory, then rename over the target.
        let tmp = path.with_extension("albumfs-tmp.png");
        img.save(&tmp)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_chunk(&self, path: &Path, _order_key: &[u8]) -> Result<(ChunkMeta, Vec<u8>), CodecError> {
        let img = image::open(path)?.to_rgba8();
        let mut src = ChannelBits::new(&img);
        let mut asm = ByteAssembler::new();

        let mut header = Vec::with_capacity(CHUNK_HEADER_LEN);
        while header.len() < CHUNK_HEADER_LEN {
            let bit = src.next().ok_or(CodecError::NotACarrier)?;
            if let Some(byte) = asm.push(bit) {
                header.push(byte);
            }
        }
        let (meta, chunk_len) = decode_header(&header)?;

        let mut payload = Vec::with_capacity(chunk_len as usize);
        while payload.len() < chunk_len as usize {
            let bit = src.next().ok_or(CodecError::NotACarrier)?;
            if let Some(byte) = asm.push(bit) {
                payload.push(byte);
            }
        }
        Ok((meta, payload))
    }
}

// Yields bits MSB first from slice `a` then slice `b`.
struct BitReader<'a> {
    a: &'a [u8],
    b: &'a [u8],
    idx: usize,
    total: usize,
}
impl<'a> BitReader<'a> {
    fn new(a: &'a [u8], b: &'a [u8]) -> Self {
        Self { a, b, idx: 0, total: (a.len() + b.len()) * 8 }
    }
    fn next(&mut self) -> Option<u8> {
        if self.idx >= self.total {
            return None;
        }
        let byte_i = self.idx / 8;
        let bit_i = 7 - (self.idx % 8);
        let byte = if byte_i < self.a.len() {
            self.a[byte_i]
        } else {
            self.b[byte_i - self.a.len()]
        };
        self.idx += 1;
        Some((byte >> bit_i) & 1)
    }
}

// Accumulates bits MSB first into whole bytes.
struct ByteAssembler {
    cur: u8,
    n: u8,
}
impl ByteAssembler {
    fn new() -> Self {
        Self { cur: 0, n: 0 }
    }
    fn push(&mut self, bit: u8) -> Option<u8> {
        self.cur = (self.cur << 1) | (bit & 1);
        self.n += 1;
        if self.n == 8 {
            let out = self.cur;
            self.cur = 0;
            self.n = 0;
            Some(out)
        } else {
            None
        }
    }
}

// Yields the LSB of each R,G,B channel in raster order.
struct ChannelBits<'a> {
    img: &'a RgbaImage,
    i: usize,
    len: usize,
    w: usize,
}
impl<'a> ChannelBits<'a> {
    fn new(img: &'a RgbaImage) -> Self {
        let (w, h) = img.dimensions();
        Self { img, i: 0, len: w as usize * h as usize * 3, w: w as usize }
    }
    fn next(&mut self) -> Option<u8> {
        if self.i >= self.len {
            return None;
        }
        let px_i = self.i / 3;
        let ch = self.i % 3;
        let x = (px_i % self.w) as u32;
        let y = (px_i / self.w) as u32;
        let p = self.img.get_pixel(x, y);
        self.i += 1;
        Some(p.0[ch] & 1)
    }
}
