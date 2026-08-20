use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{KeyInit, Tag, XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use zeroize::Zeroize;

use super::{FsError, BLOCK_HEADER, PB, UB};

pub const ARGON2_MEM_KIB: u32 = 65_536;
pub const ARGON2_ITERS: u32 = 3;
pub const ARGON2_PARALLEL: u32 = 1;
pub const ARGON2_OUTPUT_LEN: usize = 32;
pub const BOOTSTRAP_LEN: usize = 16 + 24 + 16 + UB;

const BOOT_AAD: &[u8] = b"albumfs-superblock-v1";

pub struct Key([u8; ARGON2_OUTPUT_LEN]);

impl Key {
    pub(crate) fn as_bytes(&self) -> &[u8; ARGON2_OUTPUT_LEN] {
        &self.0
    }
}

impl Zeroize for Key {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        self.zeroize();
    }
}

struct DerivedKeys {
    block: Key,
    order: Key,
    boot: Key,
}

pub(crate) struct StoreKeys {
    pub(crate) cipher: BlockCipher,
    pub(crate) order: Key,
}

pub(crate) fn create_bootstrap(
    passphrase: &str,
    encoded_superblock: &[u8],
) -> Result<([u8; BOOTSTRAP_LEN], StoreKeys), FsError> {
    if encoded_superblock.len() > UB {
        return Err(FsError::Manifest(format!(
            "encrypted superblock is {} bytes; the anchor bootstrap supports at most {UB} bytes and fewer carriers or shorter names are required",
            encoded_superblock.len()
        )));
    }
    // The salt is public and random. The fixed KDF parameters are part of the
    // encrypted format version, so changing them requires a format-version bump.
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let keys = derive_keys(passphrase, &salt)?;
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let mut ciphertext = [0u8; UB];
    ciphertext[..encoded_superblock.len()].copy_from_slice(encoded_superblock);
    let cipher =
        XChaCha20Poly1305::new_from_slice(keys.boot.as_bytes()).map_err(|_| FsError::Auth)?;
    let tag = match cipher.encrypt_in_place_detached(
        XNonce::from_slice(&nonce),
        BOOT_AAD,
        &mut ciphertext,
    ) {
        Ok(tag) => tag,
        Err(_) => {
            ciphertext.zeroize();
            return Err(FsError::Auth);
        }
    };
    let mut bootstrap = [0u8; BOOTSTRAP_LEN];
    bootstrap[..16].copy_from_slice(&salt);
    bootstrap[16..40].copy_from_slice(&nonce);
    bootstrap[40..56].copy_from_slice(&tag);
    bootstrap[56..].copy_from_slice(&ciphertext);
    ciphertext.zeroize();
    let DerivedKeys {
        block,
        order,
        boot: _,
    } = keys;
    Ok((
        bootstrap,
        StoreKeys {
            cipher: BlockCipher::new(block),
            order,
        },
    ))
}

pub(crate) fn open_bootstrap(
    passphrase: &str,
    bootstrap: &[u8],
) -> Result<([u8; UB], StoreKeys), FsError> {
    if bootstrap.len() != BOOTSTRAP_LEN {
        return Err(FsError::Auth);
    }
    let salt: [u8; 16] = bootstrap[..16].try_into().unwrap();
    let keys = derive_keys(passphrase, &salt)?;
    let mut plaintext = [0u8; UB];
    plaintext.copy_from_slice(&bootstrap[56..]);
    let cipher =
        XChaCha20Poly1305::new_from_slice(keys.boot.as_bytes()).map_err(|_| FsError::Auth)?;
    cipher
        .decrypt_in_place_detached(
            XNonce::from_slice(&bootstrap[16..40]),
            BOOT_AAD,
            &mut plaintext,
            Tag::from_slice(&bootstrap[40..56]),
        )
        .map_err(|_| {
            plaintext.zeroize();
            FsError::Auth
        })?;
    let DerivedKeys {
        block,
        order,
        boot: _,
    } = keys;
    Ok((
        plaintext,
        StoreKeys {
            cipher: BlockCipher::new(block),
            order,
        },
    ))
}

fn derive_keys(passphrase: &str, salt: &[u8; 16]) -> Result<DerivedKeys, FsError> {
    let params = Params::new(
        ARGON2_MEM_KIB,
        ARGON2_ITERS,
        ARGON2_PARALLEL,
        Some(ARGON2_OUTPUT_LEN),
    )
    .map_err(|_| FsError::Auth)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut master = [0u8; ARGON2_OUTPUT_LEN];
    if argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut master)
        .is_err()
    {
        master.zeroize();
        return Err(FsError::Auth);
    }
    let keys = DerivedKeys {
        block: Key(blake3::derive_key("albumfs block v1", &master)),
        order: Key(blake3::derive_key("albumfs order v1", &master)),
        boot: Key(blake3::derive_key("albumfs boot v1", &master)),
    };
    master.zeroize();
    Ok(keys)
}

pub struct BlockCipher {
    key: Key,
}

impl BlockCipher {
    fn new(key: Key) -> Self {
        Self { key }
    }

    pub fn encode(&self, lba: u64, payload: &[u8]) -> Result<[u8; PB], FsError> {
        if payload.len() > UB {
            return Err(FsError::Manifest(format!(
                "block payload has {} bytes, maximum is {UB}",
                payload.len()
            )));
        }

        let mut block = [0u8; PB];
        let mut plaintext = [0u8; UB];
        plaintext[..payload.len()].copy_from_slice(payload);
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let cipher =
            XChaCha20Poly1305::new_from_slice(self.key.as_bytes()).map_err(|_| FsError::Auth)?;
        let tag = match cipher.encrypt_in_place_detached(
            XNonce::from_slice(&nonce),
            &lba.to_le_bytes(),
            &mut plaintext,
        ) {
            Ok(tag) => tag,
            Err(_) => {
                plaintext.zeroize();
                return Err(FsError::Auth);
            }
        };

        block[0..24].copy_from_slice(&nonce);
        block[24..40].copy_from_slice(&tag);
        block[BLOCK_HEADER..].copy_from_slice(&plaintext);
        let mut crc = crc32fast::Hasher::new();
        crc.update(&block[BLOCK_HEADER..]);
        block[40..44].copy_from_slice(&crc.finalize().to_le_bytes());
        plaintext.zeroize();
        Ok(block)
    }

    pub fn decode(&self, lba: u64, block: &[u8]) -> Result<[u8; UB], FsError> {
        if block.len() != PB {
            return Err(FsError::Auth);
        }
        let stored_crc = u32::from_le_bytes(block[40..44].try_into().unwrap());
        let mut crc = crc32fast::Hasher::new();
        crc.update(&block[BLOCK_HEADER..]);
        if crc.finalize() != stored_crc {
            return Err(FsError::Auth);
        }

        let mut plaintext = [0u8; UB];
        plaintext.copy_from_slice(&block[BLOCK_HEADER..]);
        let cipher =
            XChaCha20Poly1305::new_from_slice(self.key.as_bytes()).map_err(|_| FsError::Auth)?;
        cipher
            .decrypt_in_place_detached(
                XNonce::from_slice(&block[0..24]),
                &lba.to_le_bytes(),
                &mut plaintext,
                Tag::from_slice(&block[24..40]),
            )
            .map_err(|_| {
                plaintext.zeroize();
                FsError::Auth
            })?;
        Ok(plaintext)
    }
}
