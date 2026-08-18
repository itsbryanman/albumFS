use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rand::Rng;

use albumfs::stego::{png::PngCodec, CarrierCodec, ChunkMeta};

#[derive(Parser)]
#[command(name = "albumfs", version, about = "AlbumFS milestone 1.1: PNG stego codec")]
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let codec = PngCodec;
    match cli.cmd {
        Cmd::Capacity { image } => {
            println!("{} usable bytes", codec.capacity_bytes(&image)?);
        }
        Cmd::CodecSelftest { image } => {
            let cap = codec.capacity_bytes(&image)?;
            let n = (cap / 2).max(1) as usize;
            let mut payload = vec![0u8; n];
            rand::thread_rng().fill(&mut payload[..]);
            let meta = ChunkMeta { chunk_index: 0, flags: 1 };
            codec.write_chunk(&image, meta, &payload, &[])?;
            let (rmeta, rpayload) = codec.read_chunk(&image, &[])?;
            if rmeta == meta && rpayload == payload {
                println!("PASS: {n} bytes round-tripped through {}", image.display());
            } else {
                eprintln!("FAIL: mismatch after round-trip");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
