use blake2::{digest::Mac, Blake2b512, Blake2bMac512, Digest};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key as ChaChaKey, XChaCha20Poly1305, XNonce,
};
use data_encoding::BASE32_NOPAD;

use crate::engine::{CryptoEngine, MAC_SIZE, NONCE_SIZE};
use crate::error::CryptoError;

pub const DIRIV_FILENAME: &str = ".aksync_diriv";
pub const MAX_RAW_FILENAME_LEN: usize = 255;
pub const LONGNAME_PREFIX: &str = "AKS_LN_";
pub const LONGNAME_SUFFIX: &str = ".name";

/// Hashes a long Base32 cipher name using Blake2b-512 with domain separator "aksync-longname-v3:",
/// taking the first 32 bytes (256-bit) and encoding to Base32 (no padding), prefixed with "AKS_LN_".
/// Result is always 59 characters, guaranteeing <= 255 bytes for filesystem safety.
pub fn hash_long_cipher_name(full_cipher_name: &str) -> String {
    let mut hasher = Blake2b512::new();
    hasher.update(b"aksync-longname-v3:");
    hasher.update(full_cipher_name.as_bytes());
    let hash = hasher.finalize();
    let b32 = BASE32_NOPAD.encode(&hash[..32]);
    format!("{}{}", LONGNAME_PREFIX, b32)
}

/// Checks if a raw filename matches the longname hashed file pattern (AKS_LN_... without .name suffix).
#[inline]
pub fn is_longname_file(raw_name: &str) -> bool {
    raw_name.starts_with(LONGNAME_PREFIX) && !raw_name.ends_with(LONGNAME_SUFFIX)
}

/// Checks if a raw filename matches the longname sidecar pattern (AKS_LN_... with .name suffix).
#[inline]
pub fn is_longname_sidecar(raw_name: &str) -> bool {
    raw_name.starts_with(LONGNAME_PREFIX) && raw_name.ends_with(LONGNAME_SUFFIX)
}

/// Returns the sidecar filename corresponding to a short raw longname file (e.g., "AKS_LN_...name").
#[inline]
pub fn longname_sidecar_for(short_raw_name: &str) -> String {
    format!("{}{}", short_raw_name, LONGNAME_SUFFIX)
}

/// Encrypts a plaintext filename or directory name using XChaCha20-Poly1305 with k_name
/// and encodes the output into filesystem-safe, case-insensitive Base32.
/// Optional dir_context (parent directory's dir_id) ensures files in different directories have distinct ciphertexts
/// and binds the filename cryptographically to its parent directory via AAD.
/// Special directory names ".", "..", ".aksync_diriv", ".nomedia", ".directory" are preserved as-is.
pub fn encrypt_filename(
    engine: &CryptoEngine,
    plain_name: &str,
    dir_context: Option<&str>,
) -> Result<String, CryptoError> {
    if plain_name == "."
        || plain_name == ".."
        || plain_name == DIRIV_FILENAME
        || plain_name == ".nomedia"
        || plain_name == ".directory"
    {
        return Ok(plain_name.to_string());
    }

    // Derive deterministic 192-bit nonce using Blake2bMac512 keyed with engine.k_name()
    let mut mac = <Blake2bMac512 as Mac>::new_from_slice(engine.k_name())
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
    mac.update(b"mycloud-filename-nonce-v3");
    if let Some(ctx) = dir_context {
        mac.update(ctx.as_bytes());
    }
    mac.update(plain_name.as_bytes());
    let hash = mac.finalize().into_bytes();

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    nonce_bytes.copy_from_slice(&hash[0..NONCE_SIZE]);

    let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(engine.k_name()));
    let nonce = XNonce::from_slice(&nonce_bytes);
    let aad = dir_context.unwrap_or("").as_bytes();
    let payload = Payload {
        msg: plain_name.as_bytes(),
        aad,
    };

    let ciphertext = cipher
        .encrypt(nonce, payload)
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

    // Combine [nonce (24 bytes)] + [ciphertext + tag]
    let mut out_bytes = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    out_bytes.extend_from_slice(&nonce_bytes);
    out_bytes.extend_from_slice(&ciphertext);

    // Encode to Base32 (no padding, case-insensitive uppercase, no special separators)
    Ok(BASE32_NOPAD.encode(&out_bytes))
}

/// Decodes and decrypts an encrypted Base32 filename back into its plaintext representation using XChaCha20-Poly1305 with k_name.
/// Verifies the AAD binding against dir_context.unwrap_or("").
/// Special directory names ".", "..", ".aksync_diriv", ".nomedia", ".directory" are preserved as-is.
pub fn decrypt_filename(
    engine: &CryptoEngine,
    cipher_name: &str,
    dir_context: Option<&str>,
) -> Result<String, CryptoError> {
    if cipher_name == "."
        || cipher_name == ".."
        || cipher_name == DIRIV_FILENAME
        || cipher_name == ".nomedia"
        || cipher_name == ".directory"
    {
        return Ok(cipher_name.to_string());
    }

    // Decode Base32
    let payload = BASE32_NOPAD
        .decode(cipher_name.as_bytes())
        .map_err(|e| CryptoError::Base32Decode(e.to_string()))?;

    // Minimum payload length: NONCE_SIZE (24) + MAC_SIZE (16)
    if payload.len() < NONCE_SIZE + MAC_SIZE {
        return Err(CryptoError::DecryptionFailed);
    }

    let nonce_bytes = &payload[..NONCE_SIZE];
    let ciphertext = &payload[NONCE_SIZE..];
    let nonce = XNonce::from_slice(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(engine.k_name()));
    let aad = dir_context.unwrap_or("").as_bytes();
    let payload_in = Payload {
        msg: ciphertext,
        aad,
    };

    let decrypted = cipher
        .decrypt(nonce, payload_in)
        .map_err(|_| CryptoError::DecryptionFailed)?;

    let plain_name = String::from_utf8(decrypted)?;
    Ok(plain_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filename_roundtrip() {
        let engine = CryptoEngine::from_key([0x42; 32]);
        let plain = "important_document.pdf";
        let dir_ctx = "0123456789abcdef0123456789abcdef0123456789abcdef";
        let encrypted = encrypt_filename(&engine, plain, Some(dir_ctx)).expect("encryption failed");
        assert_ne!(plain, encrypted);
        let decrypted =
            decrypt_filename(&engine, &encrypted, Some(dir_ctx)).expect("decryption failed");
        assert_eq!(plain, decrypted);
    }

    #[test]
    fn test_context_difference() {
        let engine = CryptoEngine::from_key([0x42; 32]);
        let plain = "same_name.txt";
        let enc1 = encrypt_filename(&engine, plain, Some("dir_a")).unwrap();
        let enc2 = encrypt_filename(&engine, plain, Some("dir_b")).unwrap();
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_special_directories() {
        let engine = CryptoEngine::from_key([0x42; 32]);
        for special in &[".", "..", DIRIV_FILENAME, ".nomedia", ".directory"] {
            assert_eq!(encrypt_filename(&engine, special, None).unwrap(), *special);
            assert_eq!(decrypt_filename(&engine, special, None).unwrap(), *special);
        }
    }

    #[test]
    fn test_filename_aad_prevents_cross_dir_move() {
        let engine = CryptoEngine::from_key([0x42; 32]);
        let plain = "secret_file.docx";
        let dir_id_a = "0123456789abcdef0123456789abcdef0123456789abcdef";
        let dir_id_b = "fedcba9876543210fedcba9876543210fedcba9876543210";

        let enc_a = encrypt_filename(&engine, plain, Some(dir_id_a)).expect("encryption failed");

        // Decrypting with wrong dir_context must fail with CryptoError::DecryptionFailed
        let res = decrypt_filename(&engine, &enc_a, Some(dir_id_b));
        assert!(matches!(res, Err(CryptoError::DecryptionFailed)));

        // Decrypting with correct dir_context succeeds
        let dec_a = decrypt_filename(&engine, &enc_a, Some(dir_id_a)).expect("decryption failed");
        assert_eq!(plain, dec_a);
    }

    #[test]
    fn test_longname_helpers() {
        let long_cipher = "A".repeat(300);
        let short = hash_long_cipher_name(&long_cipher);
        assert_eq!(short.len(), 59);
        assert!(short.starts_with(LONGNAME_PREFIX));
        assert!(is_longname_file(&short));
        assert!(!is_longname_sidecar(&short));

        let sidecar = longname_sidecar_for(&short);
        assert_eq!(sidecar, format!("{}{}", short, LONGNAME_SUFFIX));
        assert!(is_longname_sidecar(&sidecar));
        assert!(!is_longname_file(&sidecar));

        // Deterministic hash check
        let short2 = hash_long_cipher_name(&long_cipher);
        assert_eq!(short, short2);

        let diff_cipher = "B".repeat(300);
        assert_ne!(short, hash_long_cipher_name(&diff_cipher));
    }
}
