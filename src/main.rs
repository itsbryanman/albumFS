use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use rand::Rng;
use zeroize::Zeroizing;

use albumfs::fs::format::{format_with_anchor_options, load_bitmap, open, open_with_anchor};
use albumfs::fs::fuse::{mount_with_anchor, mount_with_passphrase, umount};
use albumfs::fs::stats::{
    collect as collect_stats, collect_with_anchor as collect_stats_with_anchor,
    render as render_stats,
};
use albumfs::fs::AlbumFs;
use albumfs::stego::{CarrierCodec, ChunkMeta, ChunkRead, ChunkWrite, Codec};

#[derive(Parser)]
#[command(name = "albumfs", version, about = "AlbumFS")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print usable byte capacity of a PNG carrier.
    Capacity { image: PathBuf },
    /// Round-trip a random payload through a PNG carrier (mutates the file). Reports PASS or FAIL.
    CodecSelftest { image: PathBuf },
    /// Format a directory of image carriers as an AlbumFS pool.
    Format {
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        anchor: Option<PathBuf>,
        dir: PathBuf,
    },
    /// Print the geometry and free space of an AlbumFS pool.
    Info {
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long)]
        anchor: Option<PathBuf>,
        dir: PathBuf,
    },
    /// Print allocation, inode, encryption, and per-carrier statistics.
    Stats {
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long)]
        anchor: Option<PathBuf>,
        dir: PathBuf,
    },
    /// Mount an AlbumFS pool at a directory through FUSE.
    Mount {
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long)]
        anchor: Option<PathBuf>,
        dir: PathBuf,
        mountpoint: PathBuf,
    },
    /// Unmount an AlbumFS mountpoint.
    Umount { mountpoint: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Capacity { image } => {
            let codec =
                Codec::for_path(&image).ok_or_else(|| anyhow!("unsupported carrier image type"))?;
            println!("{} usable bytes", codec.capacity_bytes(&image)?);
        }
        Cmd::CodecSelftest { image } => {
            let codec =
                Codec::for_path(&image).ok_or_else(|| anyhow!("unsupported carrier image type"))?;
            let cap = codec.capacity_bytes(&image)?;
            let n = (cap / 2).max(1) as usize;
            let mut payload = vec![0u8; n];
            rand::thread_rng().fill(&mut payload[..]);
            let meta = ChunkMeta {
                chunk_index: 0,
                flags: 1,
            };
            codec.write_chunk(&image, ChunkWrite::Framed(meta), &payload, &[])?;
            let (rmeta, rpayload) = codec.read_chunk(&image, ChunkRead::Framed, &[])?;
            if rmeta == meta && rpayload == payload {
                println!("PASS: {n} bytes round-tripped through {}", image.display());
            } else {
                eprintln!("FAIL: mismatch after round-trip");
                std::process::exit(1);
            }
        }
        Cmd::Format {
            passphrase,
            force,
            anchor,
            dir,
        } => {
            let passphrase = resolve_passphrase(passphrase)?;
            let result = format_with_anchor_options(
                &dir,
                passphrase.as_deref().map(String::as_str),
                anchor.as_deref(),
                force,
            )?;
            let sb = result.superblock;
            println!(
                "formatted {} blocks, data starts at {}, {} images, encrypted: {}",
                sb.total_blocks,
                sb.data_start,
                sb.manifest.len(),
                sb.encrypted()
            );
            if let Some(anchor) = result.anchor {
                println!("anchor: {}", anchor.display());
            }
        }
        Cmd::Info {
            passphrase,
            anchor,
            dir,
        } => {
            let passphrase = resolve_passphrase(passphrase)?;
            let (sb, mut store) = match (anchor.as_deref(), passphrase.as_deref()) {
                (Some(anchor), Some(passphrase)) => {
                    open_with_anchor(&dir, anchor, passphrase.as_str())?
                }
                (None, None) => open(&dir)?,
                _ => {
                    return Err(anyhow!(
                        "encrypted info requires both --anchor and a passphrase"
                    ))
                }
            };
            println!("block size: {}", sb.block_size);
            println!("total blocks: {}", sb.total_blocks);
            println!("inode count: {}", sb.inode_count);
            println!("image count: {}", sb.manifest.len());
            println!("encrypted: {}", sb.encrypted());
            if sb.encrypted() {
                println!("free blocks: unavailable without passphrase");
                println!("used inodes: unavailable without passphrase");
            } else {
                let bitmap = load_bitmap(&mut store, &sb)?;
                println!("free blocks: {}", bitmap.count_free());
                let mut fs = AlbumFs::open(&dir)?;
                let used_inodes = fs.used_inodes()?;
                let root_is_directory = fs.getattr(sb.root_inode)?.is_dir();
                println!("used inodes: {used_inodes}");
                println!("root present and directory: {root_is_directory}");
            }
            for carrier in &sb.manifest {
                println!(
                    "carrier {}: blocks {}..{}",
                    carrier.chunk_index,
                    carrier.block_start,
                    carrier.block_start + carrier.block_count
                );
            }
        }
        Cmd::Stats {
            passphrase,
            anchor,
            dir,
        } => {
            let passphrase = resolve_passphrase(passphrase)?;
            let stats = match (anchor.as_deref(), passphrase.as_deref()) {
                (Some(anchor), Some(passphrase)) => {
                    collect_stats_with_anchor(&dir, anchor, passphrase.as_str())?
                }
                (None, None) => collect_stats(&dir, None)?,
                _ => {
                    return Err(anyhow!(
                        "encrypted stats requires both --anchor and a passphrase"
                    ))
                }
            };
            print!("{}", render_stats(&stats));
        }
        Cmd::Mount {
            passphrase,
            anchor,
            dir,
            mountpoint,
        } => {
            let passphrase = resolve_passphrase(passphrase)?;
            match (anchor.as_deref(), passphrase.as_deref()) {
                (Some(anchor), Some(passphrase)) => {
                    mount_with_anchor(&dir, &mountpoint, anchor, passphrase.as_str())?
                }
                (None, None) => mount_with_passphrase(&dir, &mountpoint, None)?,
                _ => {
                    return Err(anyhow!(
                        "encrypted mount requires both --anchor and a passphrase"
                    ))
                }
            }
        }
        Cmd::Umount { mountpoint } => umount(&mountpoint)?,
    }
    Ok(())
}

fn resolve_passphrase(flag: Option<String>) -> Result<Option<Zeroizing<String>>> {
    if let Some(passphrase) = flag {
        return Ok(Some(Zeroizing::new(passphrase)));
    }
    match std::env::var("ALBUMFS_PASSPHRASE") {
        Ok(passphrase) => Ok(Some(Zeroizing::new(passphrase))),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(anyhow!("ALBUMFS_PASSPHRASE is not valid Unicode"))
        }
    }
}
