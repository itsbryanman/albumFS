use super::FsError;

#[derive(Debug, Clone)]
pub struct Bitmap {
    bits: Vec<u8>,
    total_blocks: u64,
}

impl Bitmap {
    pub fn new(total_blocks: u64) -> Self {
        let len = total_blocks.div_ceil(8) as usize;
        Self {
            bits: vec![0u8; len],
            total_blocks,
        }
    }

    pub fn from_bytes(bytes: &[u8], total_blocks: u64) -> Self {
        let len = total_blocks.div_ceil(8) as usize;
        let mut bits = vec![0u8; len];
        let n = len.min(bytes.len());
        bits[..n].copy_from_slice(&bytes[..n]);
        Self { bits, total_blocks }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    pub fn is_set(&self, block: u64) -> bool {
        block < self.total_blocks && (self.bits[(block / 8) as usize] >> (block % 8)) & 1 == 1
    }

    pub fn set(&mut self, block: u64) {
        if block < self.total_blocks {
            self.bits[(block / 8) as usize] |= 1 << (block % 8);
        }
    }

    pub fn clear(&mut self, block: u64) {
        if block < self.total_blocks {
            self.bits[(block / 8) as usize] &= !(1 << (block % 8));
        }
    }

    pub fn count_free(&self) -> u64 {
        (0..self.total_blocks)
            .filter(|block| !self.is_set(*block))
            .count() as u64
    }

    pub fn alloc_from(&mut self, from: u64) -> Result<u64, FsError> {
        for block in from..self.total_blocks {
            if !self.is_set(block) {
                self.set(block);
                return Ok(block);
            }
        }
        Err(FsError::NoSpace)
    }
}
