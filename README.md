# AlbumFS

A read/write filesystem that lives inside a folder of photos.

[![CI](https://github.com/itsbryanman/albumfs/actions/workflows/ci.yml/badge.svg)](https://github.com/itsbryanman/albumfs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-lightgrey.svg)](#install)
[![Status](https://img.shields.io/badge/status-alpha-red.svg)](#status)
[![Stars](https://img.shields.io/github/stars/itsbryanman/albumfs?style=social)](https://github.com/itsbryanman/albumfs/stargazers)

Point AlbumFS at a directory of pictures and it hands you back a filesystem. Write a file into it and the bytes get spread across the low bits of your photos, below the threshold your eye can see. Unmount, and the folder is just pictures again. Double-click any one of them and it opens as the same beach you shot last summer.

No cloud. No hidden partition. No container file sitting there looking suspicious. The data is in the pixels.

This is not steghide with extra steps. steghide drops one blob into one image. AlbumFS builds a real filesystem across the whole album, with directories, inodes, and free-space tracking, and lets you mount it and use it like any other disk.

## Status

Alpha. Being built in the open, one milestone at a time. Here is exactly what runs today so you are not chasing vaporware.

| Piece | State |
|---|---|
| PNG carrier codec (embed, extract, capacity) | working |
| Bit-identical round-trip, verified in tests | working |
| Filesystem layer (superblock, inodes, bitmap) | in progress, next milestone |
| FUSE mount | in progress, next milestone |
| JPEG carriers (DCT coefficient domain) | designed, not built |
| Encryption (XChaCha20-Poly1305 + Argon2id) | designed, not built |

If you clone it right now you get the codec and its test suite. The `mount` command lands in v0.2. Watch the [roadmap](#roadmap).

## How it works

Four layers stacked on top of each other. The bottom one changes per carrier format. Everything above it does not care.

1. **Stego layer.** Each photo becomes a chunk of raw capacity. For PNG that is one bit tucked into the low position of every red, green, and blue channel. Alpha is left alone. A 4000x3000 photo carries about 4.3 MB this way, and the change is invisible.
2. **Block layer.** That raw capacity gets carved into fixed 4 KiB blocks. The pool of photos becomes one flat array of blocks, and a manifest maps block numbers back to which photo they live in.
3. **Filesystem layer.** A small ext2, basically. Superblock, an inode table, a free-space bitmap, directory entries. Real files, real folders.
4. **FUSE layer.** Mounts the whole thing so your OS sees a normal drive.

Photos that do not contain a valid AlbumFS chunk are ignored. So you can salt the folder with genuine vacation photos as decoys and only some of them are actually carriers. Nothing on the outside tells you which.

## Install

No crates.io release yet. Build from source.

```sh
git clone https://github.com/itsbryanman/albumfs
cd albumfs
cargo build --release
```

The binary lands at `target/release/albumfs`.

macOS needs [macFUSE](https://osxfuse.github.io/) once the mount command ships. Linux needs `fuse3` and its headers.

## Usage

Working today:

```sh
# how many usable bytes will this photo hold
albumfs capacity beach.png

# embed a random payload, read it back, prove the round-trip
# note: this mutates the file, run it on a throwaway copy
albumfs codec-selftest ./scratch-copy.png
```

The target UX, landing in v0.2:

```sh
# turn a folder of photos into a formatted filesystem
albumfs format ./album

# mount it
albumfs mount ./album ~/vault

# now it is a normal drive
cp taxes.pdf ~/vault/
mkdir ~/vault/private
ls ~/vault

# put it away
umount ~/vault
```

## Capacity

PNG, one bit per channel, three channels:

```
usable_bytes = (width * height * 3) / 8  minus a small header
```

A 12 megapixel photo holds roughly 4.3 MB. Thirty of them give you around 126 MiB of filesystem. Plenty for documents, keys, and archives.

JPEG is a different story. You can only touch nonzero AC coefficients without wrecking the image, so a 12 MP JPEG carries tens to low hundreds of KB, not megabytes. That is the honest tradeoff for carriers that look like the JPEGs everyone actually shares.

## What this is not

Read this part. It is the difference between a fun tool and a false sense of security.

- AlbumFS hides data from someone who sees only the photos. It does not hide data from someone who holds the originals and runs a diff against them. Know which threat you actually have.
- Any program that re-saves a carrier destroys the data in it. Editors, thumbnail generators, messaging apps that recompress, and cloud sync clients all do this. If Google Photos or iCloud is syncing your carrier folder, your filesystem will evaporate on the first upload. Do not put carriers there.
- This is not a backup. There is no redundancy across photos. Lose one carrier, lose its blocks.
- JPEG capacity is small on purpose. This is a clever hiding place, not a hard drive.

Encryption, when it ships, means the carriers are unreadable and the payload is undetectable without your passphrase. Until then, treat the low bits as obfuscated, not secret.

## Roadmap

- **v0.1** PNG codec and round-trip tests. Done.
- **v0.2** Filesystem layer and FUSE mount. `format`, `mount`, `mkdir`, read, write, unmount, remount.
- **v0.3** JPEG carriers via libjpeg coefficient access, so the album can be real JPEGs.
- **v0.4** Argon2id key derivation, per-block XChaCha20-Poly1305, key-seeded embedding order.
- **v0.5** Install polish, capacity and fill stats, fuzzed on-disk parser.

## Contributing

Issues and pull requests welcome. If you break the round-trip test, that is a bug, open it. If you find a way to detect carriers that pass the current checks, absolutely open it, that is the most useful report you can file.

## License

MIT. See [LICENSE](LICENSE).

Built by [Bryan Cruse](https://github.com/itsbryanman) under Backwoods Development.
