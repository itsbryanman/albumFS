use std::fmt::Write;
use std::path::Path;

use super::block::BlockStore;
use super::format::{load_bitmap, open_with_anchor, open_with_passphrase};
use super::superblock::Superblock;
use super::{AlbumFs, FsError};

#[derive(Debug, Clone, PartialEq)]
pub struct CarrierFill {
    pub chunk_index: u32,
    pub used_blocks: u64,
    pub capacity_blocks: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FsStats {
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub used_inodes: u64,
    pub total_inodes: u64,
    pub encrypted: bool,
    pub carriers: Vec<CarrierFill>,
}

impl FsStats {
    pub fn low_space(&self) -> bool {
        self.total_blocks != 0
            && (self.free_blocks as u128) * 100 < (self.total_blocks as u128) * 10
    }
}

pub fn collect(dir: &Path, passphrase: Option<&str>) -> Result<FsStats, FsError> {
    let (sb, store) = open_with_passphrase(dir, passphrase)?;
    collect_opened(sb, store)
}

pub fn collect_with_anchor(
    dir: &Path,
    anchor: &Path,
    passphrase: &str,
) -> Result<FsStats, FsError> {
    let (sb, store) = open_with_anchor(dir, anchor, passphrase)?;
    collect_opened(sb, store)
}

fn collect_opened(sb: Superblock, mut store: BlockStore) -> Result<FsStats, FsError> {
    let bitmap = load_bitmap(&mut store, &sb)?;
    let carriers = sb
        .manifest
        .iter()
        .map(|range| CarrierFill {
            chunk_index: range.chunk_index,
            used_blocks: (range.block_start..range.block_start + range.block_count)
                .filter(|block| bitmap.is_set(*block))
                .count() as u64,
            capacity_blocks: range.block_count,
        })
        .collect();
    let free_blocks = bitmap.count_free();
    let mut fs = AlbumFs {
        store,
        sb: sb.clone(),
        bitmap,
    };
    let used_inodes = fs.used_inodes()?;
    Ok(FsStats {
        block_size: sb.block_size,
        total_blocks: sb.total_blocks,
        free_blocks,
        used_inodes,
        total_inodes: sb.inode_count,
        encrypted: sb.encrypted(),
        carriers,
    })
}

pub fn render(stats: &FsStats) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "{:<18} {}", "block size", stats.block_size);
    let _ = writeln!(output, "{:<18} {}", "total blocks", stats.total_blocks);
    let _ = writeln!(output, "{:<18} {}", "free blocks", stats.free_blocks);
    let _ = writeln!(output, "{:<18} {}", "used inodes", stats.used_inodes);
    let _ = writeln!(output, "{:<18} {}", "total inodes", stats.total_inodes);
    let _ = writeln!(
        output,
        "{:<18} {}",
        "encrypted",
        if stats.encrypted { "yes" } else { "no" }
    );
    let _ = writeln!(output, "{:<18} {}", "carrier count", stats.carriers.len());
    for carrier in &stats.carriers {
        let percentage = if carrier.capacity_blocks == 0 {
            0.0
        } else {
            carrier.used_blocks as f64 * 100.0 / carrier.capacity_blocks as f64
        };
        let _ = writeln!(
            output,
            "carrier {:<10} {} / {} blocks ({percentage:.1}%)",
            carrier.chunk_index, carrier.used_blocks, carrier.capacity_blocks
        );
    }
    if stats.low_space() {
        let _ = writeln!(output, "{:<18} free space is below 10 percent", "warning");
    }
    output
}
