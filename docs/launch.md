# Show HN: AlbumFS, a mountable filesystem hidden inside a folder of photos

AlbumFS is a Rust filesystem that spreads files and directories across the image data of ordinary PNG and JPEG photos, then mounts them through FUSE.

Suggested repository description: A mountable encrypted filesystem hidden across a folder of PNG and JPEG photos.

Suggested topics: `rust`, `filesystem`, `fuse`, `steganography`, `encryption`, `png`, `jpeg`

## Pinned first comment

The limitations first: re-encoding any carrier destroys its stored blocks, there is no redundancy if a carrier is lost, an analyst with the original photos can detect changes by comparison, and markerless mode is not a claim of resistance to statistical steganalysis.

Capacity depends heavily on the carrier. A 12 MP PNG stores about 4.3 MB because AlbumFS uses one low bit from each RGB channel. JPEG capacity is far lower, commonly tens to low hundreds of KB, because only suitable quantized AC coefficients are used.

Underneath, AlbumFS maps carrier capacity into 4 KiB blocks, builds a small inode and directory filesystem on top, and exposes it through FUSE. PNG writes change only RGB low bits; JPEG writes modify quantized coefficients and perform an entropy-only transcode.

With a passphrase, AlbumFS encrypts filesystem blocks, removes constant chunk markers, shuffles embedding positions with a derived key, and encrypts the superblock in a named anchor photo. This makes contents unreadable and their locations unlocatable without the passphrase and anchor, but it does not defeat original-image comparison or promise resistance to statistical steganalysis.
