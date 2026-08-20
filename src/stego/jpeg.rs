use std::path::Path;

use crate::stego::jpeg_ffi::{read_coefficients, write_coefficients, JpegFfiError, JpegImage};
use crate::stego::{
    decode_header, encode_header, order_positions, CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite,
    CodecError, CHUNK_HEADER_LEN,
};

pub struct JpegCodec;

impl CarrierCodec for JpegCodec {
    fn raw_capacity_bytes(&self, path: &Path) -> Result<u64, CodecError> {
        let bytes = std::fs::read(path)?;
        let image = read_coefficients(&bytes).map_err(jpeg_error)?;
        Ok(raw_capacity(&image))
    }

    fn write_chunk(
        &self,
        path: &Path,
        mode: ChunkWrite,
        payload: &[u8],
        order_key: &[u8],
    ) -> Result<(), CodecError> {
        let original = std::fs::read(path)?;
        let mut image = read_coefficients(&original).map_err(jpeg_error)?;
        let raw = raw_capacity(&image);
        let (header, positions) = match mode {
            ChunkWrite::Framed(meta) => {
                if !order_key.is_empty() {
                    return Err(CodecError::InvalidMode(
                        "framed mode requires an empty order key".into(),
                    ));
                }
                if payload.len() > u32::MAX as usize
                    || CHUNK_HEADER_LEN as u64 + payload.len() as u64 > raw
                {
                    return Err(CodecError::PayloadTooLarge {
                        payload: payload.len() as u64,
                        capacity: raw.saturating_sub(CHUNK_HEADER_LEN as u64),
                    });
                }
                (
                    Some(encode_header(meta, payload.len() as u32)),
                    usable_positions(&image),
                )
            }
            ChunkWrite::Markerless { chunk_index } => {
                if payload.len() as u64 > raw {
                    return Err(CodecError::PayloadTooLarge {
                        payload: payload.len() as u64,
                        capacity: raw,
                    });
                }
                (
                    None,
                    order_positions(usable_positions(&image), order_key, chunk_index)?,
                )
            }
        };
        let header = header.as_ref().map_or(&[][..], |bytes| &bytes[..]);
        embed_bytes(&mut image, &positions, header, payload)?;

        let encoded = write_coefficients(&original, &image).map_err(jpeg_error)?;
        let tmp = path.with_extension("albumfs-tmp.jpg");
        std::fs::write(&tmp, encoded)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_chunk(
        &self,
        path: &Path,
        mode: ChunkRead,
        order_key: &[u8],
    ) -> Result<(ChunkMeta, Vec<u8>), CodecError> {
        let bytes = std::fs::read(path)?;
        let image = read_coefficients(&bytes).map_err(jpeg_error)?;
        match mode {
            ChunkRead::Framed => {
                if !order_key.is_empty() {
                    return Err(CodecError::InvalidMode(
                        "framed mode requires an empty order key".into(),
                    ));
                }
                let positions = usable_positions(&image);
                let header = extract_bytes(&image, &positions, 0, CHUNK_HEADER_LEN)?;
                let (meta, payload_len) = decode_header(&header)?;
                if u64::from(payload_len)
                    > raw_capacity(&image).saturating_sub(CHUNK_HEADER_LEN as u64)
                {
                    return Err(CodecError::NotACarrier);
                }
                let payload = extract_bytes(
                    &image,
                    &positions,
                    CHUNK_HEADER_LEN * 8,
                    payload_len as usize,
                )?;
                Ok((meta, payload))
            }
            ChunkRead::Markerless {
                chunk_index,
                payload_len,
            } => {
                if payload_len as u64 > raw_capacity(&image) {
                    return Err(CodecError::NotACarrier);
                }
                let positions = order_positions(usable_positions(&image), order_key, chunk_index)?;
                let payload = extract_bytes(&image, &positions, 0, payload_len)?;
                Ok((
                    ChunkMeta {
                        chunk_index,
                        flags: 0,
                    },
                    payload,
                ))
            }
        }
    }

    fn write_prefix(&self, path: &Path, bytes: &[u8]) -> Result<(), CodecError> {
        let original = std::fs::read(path)?;
        let mut image = read_coefficients(&original).map_err(jpeg_error)?;
        let raw = raw_capacity(&image);
        if bytes.len() as u64 > raw {
            return Err(CodecError::PayloadTooLarge {
                payload: bytes.len() as u64,
                capacity: raw,
            });
        }
        let positions = usable_positions(&image);
        embed_bytes(&mut image, &positions, &[], bytes)?;
        let encoded = write_coefficients(&original, &image).map_err(jpeg_error)?;
        let tmp = path.with_extension("albumfs-tmp.jpg");
        std::fs::write(&tmp, encoded)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_prefix(&self, path: &Path, length: usize) -> Result<Vec<u8>, CodecError> {
        let bytes = std::fs::read(path)?;
        let image = read_coefficients(&bytes).map_err(jpeg_error)?;
        if length as u64 > raw_capacity(&image) {
            return Err(CodecError::NotACarrier);
        }
        extract_bytes(&image, &usable_positions(&image), 0, length)
    }
}

fn raw_capacity(image: &JpegImage) -> u64 {
    usable_positions(image).len() as u64 / 8
}

fn usable_positions(image: &JpegImage) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut global_block = 0usize;
    for component in &image.components {
        for block in component {
            for (index, coefficient) in block.iter().enumerate().skip(1) {
                if (i32::from(*coefficient)).unsigned_abs() >= 2 {
                    positions.push(global_block * 64 + index);
                }
            }
            global_block += 1;
        }
    }
    positions
}

fn coefficient(image: &JpegImage, position: usize) -> Option<i16> {
    let mut block_index = position / 64;
    let coefficient_index = position % 64;
    for component in &image.components {
        if block_index < component.len() {
            return component
                .get(block_index)
                .and_then(|block| block.get(coefficient_index))
                .copied();
        }
        block_index -= component.len();
    }
    None
}

fn coefficient_mut(image: &mut JpegImage, position: usize) -> Option<&mut i16> {
    let mut block_index = position / 64;
    let coefficient_index = position % 64;
    for component in &mut image.components {
        if block_index < component.len() {
            return component
                .get_mut(block_index)
                .and_then(|block| block.get_mut(coefficient_index));
        }
        block_index -= component.len();
    }
    None
}

fn embed_bytes(
    image: &mut JpegImage,
    positions: &[usize],
    first: &[u8],
    second: &[u8],
) -> Result<(), CodecError> {
    let total_bits = first
        .len()
        .checked_add(second.len())
        .and_then(|length| length.checked_mul(8))
        .ok_or_else(|| CodecError::InvalidMode("payload bit length overflow".into()))?;
    if positions.len() < total_bits {
        return Err(CodecError::PayloadTooLarge {
            payload: second.len() as u64,
            capacity: positions.len() as u64 / 8,
        });
    }
    for (bit_index, position) in positions.iter().copied().take(total_bits).enumerate() {
        let byte_index = bit_index / 8;
        let bit_in_byte = 7 - bit_index % 8;
        let byte = if byte_index < first.len() {
            first[byte_index]
        } else {
            second[byte_index - first.len()]
        };
        let bit = (byte >> bit_in_byte) & 1;
        let value = coefficient_mut(image, position).ok_or_else(|| {
            CodecError::Jpeg("usable coefficient position went out of range".into())
        })?;
        let signed = i32::from(*value);
        let magnitude = signed.unsigned_abs() as i32;
        let embedded = (magnitude & !1) | i32::from(bit);
        *value = if signed < 0 { -embedded } else { embedded } as i16;
    }
    Ok(())
}

fn extract_bytes(
    image: &JpegImage,
    positions: &[usize],
    bit_offset: usize,
    length: usize,
) -> Result<Vec<u8>, CodecError> {
    let bit_length = length
        .checked_mul(8)
        .ok_or_else(|| CodecError::InvalidMode("payload bit length overflow".into()))?;
    let end = bit_offset
        .checked_add(bit_length)
        .ok_or_else(|| CodecError::InvalidMode("payload bit range overflow".into()))?;
    if end > positions.len() {
        return Err(CodecError::NotACarrier);
    }
    let mut bytes = vec![0u8; length];
    for (output_bit, position) in positions[bit_offset..end].iter().copied().enumerate() {
        let value = coefficient(image, position).ok_or_else(|| {
            CodecError::Jpeg("usable coefficient position went out of range".into())
        })?;
        let bit = (i32::from(value).unsigned_abs() & 1) as u8;
        bytes[output_bit / 8] |= bit << (7 - output_bit % 8);
    }
    Ok(bytes)
}

fn jpeg_error(error: JpegFfiError) -> CodecError {
    CodecError::Jpeg(error.0)
}
