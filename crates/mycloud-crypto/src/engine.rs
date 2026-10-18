use argon2::{Algorithm, Argon2, Params, Version};
use blake2::{digest::Mac, Blake2b512, Blake2bMac512, Digest};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key as ChaChaKey, XChaCha20Poly1305, XNonce,
};
use crypto_secretbox::{
    aead::Aead as SalsaAead, Key as SalsaKey, Nonce as SalsaNonce, XSalsa20Poly1305,
};
use rand::RngCore;
use zeroize::Zeroize;

use crate::error::CryptoError;

pub const CHUNK_PLAIN_SIZE: usize = 65536; // 64 KB
pub const MAC_SIZE: usize = 16; // Poly1305 MAC tag
pub const CHUNK_CIPHER_SIZE: usize = CHUNK_PLAIN_SIZE + MAC_SIZE; // 65552 bytes (v2)
pub const CHUNK_CIPHER_SIZE_V2: usize = CHUNK_CIPHER_SIZE;
pub const HEADER_SIZE: usize = 24; // 192-bit base nonce (v2)
pub const HEADER_SIZE_V2: usize = HEADER_SIZE;
pub const KEY_SIZE: usize = 32; // 256-bit key
pub const NONCE_SIZE: usize = 24; // 192-bit nonce

// v3 Constants
pub const MAGIC_V3: &[u8; 4] = b"AKS3";
pub const VERSION_V3: u8 = 3;
pub const FILE_ID_SIZE: usize = 24;
pub const HEADER_TAG_SIZE: usize = 32;
pub const HEADER_SIZE_V3: usize = 64; // 4 (magic) + 1 (ver) + 3 (flags) + 24 (file_id) + 32 (hdr_tag)
pub const CHUNK_NONCE_SIZE_V3: usize = 24;
pub const CHUNK_OVERHEAD_V3: usize = CHUNK_NONCE_SIZE_V3 + MAC_SIZE; // 24 + 16 = 40
pub const CHUNK_DISK_SIZE_V3: usize = CHUNK_PLAIN_SIZE + CHUNK_OVERHEAD_V3; // 65536 + 40 = 65576

pub const ARGON2_MEMORY_KIB: u32 = 64 * 1024; // 64 MB
pub const ARGON2_ITERATIONS: u32 = 3;
pub const ARGON2_PARALLELISM: u32 = 4;

/// Subkey derivation routine using Blake2bMac512
fn derive_subkey(master_key: &[u8; KEY_SIZE], context: &[u8]) -> [u8; KEY_SIZE] {
    let mut mac = <Blake2bMac512 as Mac>::new_from_slice(master_key)
        .expect("32-byte key is valid for Blake2bMac512");
    mac.update(context);
    let mut full_output = mac.finalize().into_bytes();
    let mut subkey = [0u8; KEY_SIZE];
    subkey.copy_from_slice(&full_output[..KEY_SIZE]);
    full_output.zeroize();
    subkey
}

/// Key derivation routines using Argon2id.
pub struct KeyDerivation;

impl KeyDerivation {
    /// Generates a cryptographically secure random 16-byte salt.
    pub fn generate_salt() -> [u8; 16] {
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        salt
    }

    /// Derives a 256-bit cryptographic key from a passphrase and salt using Argon2id
    /// (v13, 64MB Memory, 3 Iterations, 4 Parallelism).
    pub fn derive_key(
        passphrase: &str,
        salt: &[u8],
    ) -> Result<zeroize::Zeroizing<[u8; KEY_SIZE]>, CryptoError> {
        let params = Params::new(
            ARGON2_MEMORY_KIB,
            ARGON2_ITERATIONS,
            ARGON2_PARALLELISM,
            Some(KEY_SIZE),
        )
        .map_err(|e| CryptoError::Argon2(e.to_string()))?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = zeroize::Zeroizing::new([0u8; KEY_SIZE]);
        argon2
            .hash_password_into(passphrase.as_bytes(), salt, &mut *key)
            .map_err(|e| CryptoError::Argon2(e.to_string()))?;

        Ok(key)
    }

    /// Generates a cryptographically secure random 32-byte Data Master Key (DMK).
    pub fn generate_master_key() -> zeroize::Zeroizing<[u8; KEY_SIZE]> {
        let mut dmk = zeroize::Zeroizing::new([0u8; KEY_SIZE]);
        rand::rngs::OsRng.fill_bytes(&mut *dmk);
        dmk
    }

    /// Generates a cryptographically secure 52-character uppercase Base32 recovery key.
    pub fn generate_recovery_key() -> zeroize::Zeroizing<String> {
        let mut entropy = zeroize::Zeroizing::new([0u8; KEY_SIZE]);
        rand::rngs::OsRng.fill_bytes(&mut *entropy);
        let encoded = data_encoding::BASE32_NOPAD.encode(&*entropy);
        zeroize::Zeroizing::new(encoded)
    }

    /// Derives a 32-byte Key Encryption Key (KEK) from a Base32 recovery key string.
    /// Strips spaces/dashes and verifies that the decoded length is exactly 32 bytes.
    pub fn derive_kek_from_recovery(
        recovery_key: &str,
    ) -> Result<zeroize::Zeroizing<[u8; KEY_SIZE]>, CryptoError> {
        let mut filtered: String = recovery_key
            .chars()
            .filter(|c| *c != '-' && !c.is_ascii_whitespace())
            .collect();
        let mut upper = filtered.to_ascii_uppercase();
        filtered.zeroize();

        let mut raw_bytes = data_encoding::BASE32_NOPAD
            .decode(upper.as_bytes())
            .map_err(|_| {
                upper.zeroize();
                CryptoError::DecryptionFailed
            })?;
        upper.zeroize();

        if raw_bytes.len() != KEY_SIZE {
            raw_bytes.zeroize();
            return Err(CryptoError::DecryptionFailed);
        }

        let mut raw_arr = [0u8; KEY_SIZE];
        raw_arr.copy_from_slice(&raw_bytes);
        raw_bytes.zeroize();

        let subkey = derive_subkey(&raw_arr, b"aksync-v3-recovery-kek");
        raw_arr.zeroize();

        Ok(zeroize::Zeroizing::new(subkey))
    }

    /// Wraps a 32-byte Data Master Key (DMK) with a 32-byte Key Encryption Key (KEK).
    /// Returns a 144-character hex string encoding [nonce (24 B)] || [ciphertext + Poly1305 tag (48 B)].
    pub fn wrap_key(
        kek: &[u8; KEY_SIZE],
        dmk: &[u8; KEY_SIZE],
    ) -> Result<String, CryptoError> {
        let mut k_wrap = derive_subkey(kek, b"aksync-v3-key-wrap");
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

        let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(&k_wrap));
        k_wrap.zeroize();

        let nonce = XNonce::from_slice(&nonce_bytes);
        let payload = Payload {
            msg: dmk.as_slice(),
            aad: b"aksync-v3-wrapped-dmk",
        };

        let encrypted = cipher
            .encrypt(nonce, payload)
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

        let mut combined = Vec::with_capacity(NONCE_SIZE + encrypted.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&encrypted);

        Ok(hex::encode(&combined))
    }

    /// Unwraps a 144-character hex string wrapped DMK using a 32-byte Key Encryption Key (KEK).
    pub fn unwrap_key(
        kek: &[u8; KEY_SIZE],
        wrapped_hex: &str,
    ) -> Result<zeroize::Zeroizing<[u8; KEY_SIZE]>, CryptoError> {
        let mut raw = hex::decode(wrapped_hex.trim())
            .map_err(|_| CryptoError::DecryptionFailed)?;

        if raw.len() != NONCE_SIZE + KEY_SIZE + MAC_SIZE {
            raw.zeroize();
            return Err(CryptoError::DecryptionFailed);
        }

        let nonce_bytes = &raw[..NONCE_SIZE];
        let ciphertext = &raw[NONCE_SIZE..];

        let mut k_wrap = derive_subkey(kek, b"aksync-v3-key-wrap");
        let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(&k_wrap));
        k_wrap.zeroize();

        let nonce = XNonce::from_slice(nonce_bytes);
        let payload = Payload {
            msg: ciphertext,
            aad: b"aksync-v3-wrapped-dmk",
        };

        let mut decrypted = cipher
            .decrypt(nonce, payload)
            .map_err(|_| {
                raw.zeroize();
                CryptoError::DecryptionFailed
            })?;
        raw.zeroize();

        if decrypted.len() != KEY_SIZE {
            decrypted.zeroize();
            return Err(CryptoError::DecryptionFailed);
        }

        let mut dmk = zeroize::Zeroizing::new([0u8; KEY_SIZE]);
        dmk.copy_from_slice(&decrypted);
        decrypted.zeroize();

        Ok(dmk)
    }
}

/// Cryptographic engine utilizing XChaCha20-Poly1305 (v3) and XSalsa20-Poly1305 (v2 fallback).
pub struct CryptoEngine {
    master_key: [u8; KEY_SIZE],
    k_data: [u8; KEY_SIZE],
    k_hdr: [u8; KEY_SIZE],
    k_name: [u8; KEY_SIZE],
    k_canary: [u8; KEY_SIZE],
}

impl CryptoEngine {
    /// Creates a new engine instance with the given 256-bit key and derives subkeys.
    pub fn from_key(master_key: [u8; KEY_SIZE]) -> Self {
        let k_data = derive_subkey(&master_key, b"aksync-v3-data");
        let k_hdr = derive_subkey(&master_key, b"aksync-v3-hdr");
        let k_name = derive_subkey(&master_key, b"aksync-v3-name");
        let k_canary = derive_subkey(&master_key, b"aksync-v3-canary");

        Self {
            master_key,
            k_data,
            k_hdr,
            k_name,
            k_canary,
        }
    }

    /// Creates a new engine instance derived from a passphrase and salt.
    pub fn from_passphrase(passphrase: &str, salt: &[u8]) -> Result<Self, CryptoError> {
        let key = KeyDerivation::derive_key(passphrase, salt)?;
        Ok(Self::from_key(*key))
    }

    /// Returns a reference to the master 256-bit key.
    pub fn key(&self) -> &[u8; KEY_SIZE] {
        &self.master_key
    }

    /// Returns a reference to the master 256-bit key.
    pub fn master_key(&self) -> &[u8; KEY_SIZE] {
        &self.master_key
    }

    /// Returns a reference to the data subkey (k_data).
    pub fn k_data(&self) -> &[u8; KEY_SIZE] {
        &self.k_data
    }

    /// Returns a reference to the header subkey (k_hdr).
    pub fn k_hdr(&self) -> &[u8; KEY_SIZE] {
        &self.k_hdr
    }

    /// Returns a reference to the filename subkey (k_name).
    pub fn k_name(&self) -> &[u8; KEY_SIZE] {
        &self.k_name
    }

    /// Returns a reference to the canary subkey (k_canary).
    pub fn k_canary(&self) -> &[u8; KEY_SIZE] {
        &self.k_canary
    }

    // ==========================================
    // Format v3: Header Operations
    // ==========================================

    /// Creates a 64-byte Format v3 file header with a freshly generated random file_id.
    pub fn create_v3_header(&self) -> ([u8; HEADER_SIZE_V3], [u8; FILE_ID_SIZE]) {
        let mut header = [0u8; HEADER_SIZE_V3];
        header[0..4].copy_from_slice(MAGIC_V3);
        header[4] = VERSION_V3;
        header[5..8].copy_from_slice(&[0, 0, 0]);

        let mut file_id = [0u8; FILE_ID_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut file_id);
        header[8..32].copy_from_slice(&file_id);

        let mut mac =
            <Blake2bMac512 as Mac>::new_from_slice(&self.k_hdr).expect("valid key len");
        mac.update(&header[0..32]);
        let tag = mac.finalize().into_bytes();
        header[32..64].copy_from_slice(&tag[0..HEADER_TAG_SIZE]);

        (header, file_id)
    }

    /// Verifies a 64-byte Format v3 header in constant-time and extracts the authenticated file_id.
    pub fn verify_v3_header(&self, header: &[u8]) -> Result<[u8; FILE_ID_SIZE], CryptoError> {
        if header.len() != HEADER_SIZE_V3 {
            return Err(CryptoError::InvalidHeader(format!(
                "Header length {} != {}",
                header.len(),
                HEADER_SIZE_V3
            )));
        }
        if &header[0..4] != MAGIC_V3 {
            return Err(CryptoError::InvalidHeader("Invalid magic bytes".into()));
        }
        if header[4] != VERSION_V3 {
            return Err(CryptoError::InvalidHeader(format!(
                "Unsupported header version {}",
                header[4]
            )));
        }
        if header[5..8] != [0, 0, 0] {
            return Err(CryptoError::InvalidHeader("Invalid header flags".into()));
        }

        let mut mac =
            <Blake2bMac512 as Mac>::new_from_slice(&self.k_hdr).expect("valid key len");
        mac.update(&header[0..32]);
        let expected_tag = mac.finalize().into_bytes();

        let mut diff = 0u8;
        for (a, b) in expected_tag[0..HEADER_TAG_SIZE].iter().zip(&header[32..64]) {
            diff |= a ^ b;
        }
        if diff != 0 {
            return Err(CryptoError::DecryptionFailed);
        }

        let mut file_id = [0u8; FILE_ID_SIZE];
        file_id.copy_from_slice(&header[8..32]);
        Ok(file_id)
    }

    /// Checks if a prefix starts with the Format v3 magic bytes "AKS3".
    #[inline]
    pub fn is_v3_header(prefix: &[u8]) -> bool {
        prefix.len() >= 4 && &prefix[0..4] == MAGIC_V3
    }

    // ==========================================
    // Format v3: Chunk Encryption & Decryption
    // ==========================================

    /// Builds the 33-byte AAD binding: file_id (24 B) || chunk_index (8 B LE) || is_last (1 B)
    pub fn build_v3_aad(file_id: &[u8; FILE_ID_SIZE], chunk_index: u64, is_last: bool) -> [u8; 33] {
        let mut aad = [0u8; 33];
        aad[0..24].copy_from_slice(file_id);
        aad[24..32].copy_from_slice(&chunk_index.to_le_bytes());
        aad[32] = if is_last { 1u8 } else { 0u8 };
        aad
    }

    /// Encrypts a chunk in Format v3 with XChaCha20-Poly1305, fresh random 24-byte nonce, and AAD.
    /// Output layout: [chunk_nonce (24 B)] || [ciphertext + Poly1305 tag (plaintext.len() + 16 B)]
    pub fn encrypt_chunk_v3(
        &self,
        file_id: &[u8; FILE_ID_SIZE],
        chunk_index: u64,
        is_last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if plaintext.len() > CHUNK_PLAIN_SIZE {
            return Err(CryptoError::InvalidHeader(format!(
                "Plaintext chunk exceeds maximum allowed chunk size: {} > {}",
                plaintext.len(),
                CHUNK_PLAIN_SIZE
            )));
        }

        let mut chunk_nonce = [0u8; CHUNK_NONCE_SIZE_V3];
        rand::rngs::OsRng.fill_bytes(&mut chunk_nonce);

        let aad = Self::build_v3_aad(file_id, chunk_index, is_last);
        let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(&self.k_data));
        let nonce = XNonce::from_slice(&chunk_nonce);
        let payload = Payload {
            msg: plaintext,
            aad: &aad,
        };

        let ciphertext = cipher
            .encrypt(nonce, payload)
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

        let mut out = Vec::with_capacity(CHUNK_NONCE_SIZE_V3 + ciphertext.len());
        out.extend_from_slice(&chunk_nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypts a chunk in Format v3 with XChaCha20-Poly1305, verifying AAD and Poly1305 tag.
    pub fn decrypt_chunk_v3(
        &self,
        file_id: &[u8; FILE_ID_SIZE],
        chunk_index: u64,
        expected_is_last: bool,
        raw_chunk: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if raw_chunk.len() < CHUNK_OVERHEAD_V3 || raw_chunk.len() > CHUNK_DISK_SIZE_V3 {
            return Err(CryptoError::DecryptionFailed);
        }

        let chunk_nonce = &raw_chunk[..CHUNK_NONCE_SIZE_V3];
        let ciphertext = &raw_chunk[CHUNK_NONCE_SIZE_V3..];

        let aad = Self::build_v3_aad(file_id, chunk_index, expected_is_last);
        let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(&self.k_data));
        let nonce = XNonce::from_slice(chunk_nonce);
        let payload = Payload {
            msg: ciphertext,
            aad: &aad,
        };

        let plaintext = cipher
            .decrypt(nonce, payload)
            .map_err(|_| CryptoError::DecryptionFailed)?;

        Ok(plaintext)
    }

    // ==========================================
    // Format v3: Size & Offset Calculations
    // ==========================================

    /// Translates raw v3 ciphertext file size to decrypted plaintext file size, returning an error if corrupted/truncated.
    pub fn calculate_plain_size_v3_checked(raw_file_size: u64) -> Result<u64, CryptoError> {
        if raw_file_size < HEADER_SIZE_V3 as u64 {
            return Err(CryptoError::InvalidHeader(
                "File smaller than v3 header".into(),
            ));
        }
        if raw_file_size == HEADER_SIZE_V3 as u64 {
            return Ok(0);
        }

        let payload = raw_file_size - HEADER_SIZE_V3 as u64;
        let full_chunks = payload / (CHUNK_DISK_SIZE_V3 as u64);
        let remainder = payload % (CHUNK_DISK_SIZE_V3 as u64);

        if remainder == 0 {
            Ok(full_chunks * (CHUNK_PLAIN_SIZE as u64))
        } else if remainder <= CHUNK_OVERHEAD_V3 as u64 {
            Err(CryptoError::DecryptionFailed)
        } else {
            Ok(full_chunks * (CHUNK_PLAIN_SIZE as u64) + (remainder - CHUNK_OVERHEAD_V3 as u64))
        }
    }

    /// Returns the total number of chunks for a raw v3 ciphertext file size.
    pub fn total_chunks_v3_checked(raw_file_size: u64) -> Result<u64, CryptoError> {
        if raw_file_size < HEADER_SIZE_V3 as u64 {
            return Err(CryptoError::InvalidHeader(
                "File smaller than v3 header".into(),
            ));
        }
        if raw_file_size == HEADER_SIZE_V3 as u64 {
            return Ok(0);
        }

        let payload = raw_file_size - HEADER_SIZE_V3 as u64;
        let full_chunks = payload / (CHUNK_DISK_SIZE_V3 as u64);
        let remainder = payload % (CHUNK_DISK_SIZE_V3 as u64);

        if remainder == 0 {
            Ok(full_chunks)
        } else if remainder <= CHUNK_OVERHEAD_V3 as u64 {
            Err(CryptoError::DecryptionFailed)
        } else {
            Ok(full_chunks + 1)
        }
    }

    /// Returns the raw ciphertext file byte offset where chunk `chunk_index` begins in Format v3.
    #[inline]
    pub fn chunk_offset_to_cipher_offset_v3(chunk_index: u64) -> u64 {
        (HEADER_SIZE_V3 as u64) + chunk_index * (CHUNK_DISK_SIZE_V3 as u64)
    }

    /// Returns the chunk index containing the given plaintext byte offset in Format v3.
    #[inline]
    pub fn plain_offset_to_chunk_index_v3(plain_offset: u64) -> u64 {
        plain_offset / (CHUNK_PLAIN_SIZE as u64)
    }

    /// Returns the byte offset within the chunk for the given plaintext byte offset in Format v3.
    #[inline]
    pub fn plain_offset_in_chunk_v3(plain_offset: u64) -> usize {
        (plain_offset % (CHUNK_PLAIN_SIZE as u64)) as usize
    }

    // ==========================================
    // Legacy Format v2: Operations (for Migration & Fallback)
    // ==========================================

    /// Generates a random 192-bit Base Nonce to be stored in the file header (v2).
    pub fn generate_base_nonce() -> [u8; NONCE_SIZE] {
        let mut nonce = [0u8; NONCE_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        nonce
    }

    /// Derives a 192-bit chunk nonce cryptographically from the base nonce and chunk index using BLAKE2b (v2).
    pub fn derive_chunk_nonce(base_nonce: &[u8; NONCE_SIZE], chunk_index: u64) -> [u8; NONCE_SIZE] {
        let mut hasher = Blake2b512::new();
        blake2::Digest::update(&mut hasher, b"mycloud-chunk-nonce-v2");
        blake2::Digest::update(&mut hasher, base_nonce);
        blake2::Digest::update(&mut hasher, chunk_index.to_le_bytes());
        let hash = blake2::Digest::finalize(hasher);
        let mut nonce = [0u8; NONCE_SIZE];
        nonce.copy_from_slice(&hash[0..NONCE_SIZE]);
        nonce
    }

    /// Encrypts a chunk payload (up to 64 KB) using XSalsa20-Poly1305 with master_key (v2).
    pub fn encrypt_chunk(
        &self,
        chunk_nonce: &[u8; NONCE_SIZE],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if plaintext.len() > CHUNK_PLAIN_SIZE {
            return Err(CryptoError::InvalidHeader(format!(
                "Plaintext chunk exceeds maximum allowed chunk size: {} > {}",
                plaintext.len(),
                CHUNK_PLAIN_SIZE
            )));
        }

        let cipher = XSalsa20Poly1305::new(SalsaKey::from_slice(&self.master_key));
        let nonce = SalsaNonce::from_slice(chunk_nonce);
        SalsaAead::encrypt(&cipher, nonce, plaintext)
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))
    }

    /// Decrypts a chunk payload (up to 65552 bytes) using XSalsa20-Poly1305 with master_key (v2).
    pub fn decrypt_chunk(
        &self,
        chunk_nonce: &[u8; NONCE_SIZE],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.len() < MAC_SIZE {
            return Err(CryptoError::DecryptionFailed);
        }
        if ciphertext.len() > CHUNK_CIPHER_SIZE {
            return Err(CryptoError::InvalidHeader(format!(
                "Ciphertext chunk exceeds maximum chunk size: {} > {}",
                ciphertext.len(),
                CHUNK_CIPHER_SIZE
            )));
        }

        let cipher = XSalsa20Poly1305::new(SalsaKey::from_slice(&self.master_key));
        let nonce = SalsaNonce::from_slice(chunk_nonce);
        SalsaAead::decrypt(&cipher, nonce, ciphertext).map_err(|_| CryptoError::DecryptionFailed)
    }

    // ==========================================
    // Vault Verification Token (Canary)
    // ==========================================

    /// Generates a verification token (hex-encoded ciphertext of a known canary) to verify password correctness upon unlock.
    /// Encrypted with k_canary using XChaCha20-Poly1305.
    pub fn generate_vault_verification_token(&self) -> Result<String, CryptoError> {
        const CANARY_NONCE: [u8; NONCE_SIZE] = *b"mycloud-canary-nonce-24b";
        const CANARY_PLAINTEXT: &[u8] = b"mycloud-vault-verified-v1";
        let cipher = XChaCha20Poly1305::new(ChaChaKey::from_slice(&self.k_canary));
        let nonce = XNonce::from_slice(&CANARY_NONCE);
        let payload = Payload {
            msg: CANARY_PLAINTEXT,
            aad: b"aksync-canary-aad",
        };
        let encrypted = cipher
            .encrypt(nonce, payload)
            .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
        Ok(data_encoding::HEXLOWER.encode(&encrypted))
    }

    /// Verifies if a given hex-encoded token matches the canary under this engine's key.
    /// Tries k_canary (v3) first, then falls back to master_key with XSalsa20Poly1305 (v2).
    pub fn verify_vault_token(&self, token_hex: &str) -> bool {
        const CANARY_NONCE: [u8; NONCE_SIZE] = *b"mycloud-canary-nonce-24b";
        const CANARY_PLAINTEXT: &[u8] = b"mycloud-vault-verified-v1";

        let bytes = match data_encoding::HEXLOWER.decode(token_hex.trim().as_bytes()) {
            Ok(b) => b,
            Err(_) => return false,
        };

        // 1. Try v3 with k_canary + XChaCha20Poly1305
        let cipher_v3 = XChaCha20Poly1305::new(ChaChaKey::from_slice(&self.k_canary));
        let nonce_v3 = XNonce::from_slice(&CANARY_NONCE);
        let payload_v3 = Payload {
            msg: &bytes,
            aad: b"aksync-canary-aad",
        };
        if let Ok(decrypted) = cipher_v3.decrypt(nonce_v3, payload_v3) {
            if decrypted == CANARY_PLAINTEXT {
                return true;
            }
        }

        // 2. Try v2 fallback with master_key + XSalsa20Poly1305
        let cipher_v2 = XSalsa20Poly1305::new(SalsaKey::from_slice(&self.master_key));
        let nonce_v2 = SalsaNonce::from_slice(&CANARY_NONCE);
        if let Ok(decrypted) = SalsaAead::decrypt(&cipher_v2, nonce_v2, bytes.as_slice()) {
            if decrypted == CANARY_PLAINTEXT {
                return true;
            }
        }

        false
    }

    // ==========================================
    // Legacy Format v2: Size Calculations
    // ==========================================

    /// Translates raw ciphertext file size to decrypted plaintext file size (v2).
    pub fn calculate_plain_size(raw_file_size: u64) -> u64 {
        if raw_file_size <= HEADER_SIZE as u64 {
            return 0;
        }

        let payload_size = raw_file_size - HEADER_SIZE as u64;
        let full_chunks = payload_size / (CHUNK_CIPHER_SIZE as u64);
        let remainder = payload_size % (CHUNK_CIPHER_SIZE as u64);

        if remainder == 0 {
            full_chunks * (CHUNK_PLAIN_SIZE as u64)
        } else if remainder > MAC_SIZE as u64 {
            (full_chunks * (CHUNK_PLAIN_SIZE as u64)) + (remainder - MAC_SIZE as u64)
        } else {
            // Remainder contains only MAC or is incomplete/corrupted
            full_chunks * (CHUNK_PLAIN_SIZE as u64)
        }
    }

    /// Translates plaintext file size to encrypted raw file size (v2).
    pub fn calculate_raw_size(plain_size: u64) -> u64 {
        if plain_size == 0 {
            return HEADER_SIZE as u64;
        }

        let full_chunks = plain_size / (CHUNK_PLAIN_SIZE as u64);
        let remainder = plain_size % (CHUNK_PLAIN_SIZE as u64);

        if remainder == 0 {
            (HEADER_SIZE as u64) + (full_chunks * (CHUNK_CIPHER_SIZE as u64))
        } else {
            (HEADER_SIZE as u64)
                + (full_chunks * (CHUNK_CIPHER_SIZE as u64))
                + (remainder + MAC_SIZE as u64)
        }
    }

    /// Returns the raw ciphertext file byte offset where chunk `chunk_index` begins (v2).
    #[inline]
    pub fn chunk_offset_to_cipher_offset(chunk_index: u64) -> u64 {
        (HEADER_SIZE as u64) + (chunk_index * (CHUNK_CIPHER_SIZE as u64))
    }

    /// Returns the chunk index containing the given plaintext byte offset (v2).
    #[inline]
    pub fn plain_offset_to_chunk_index(plain_offset: u64) -> u64 {
        plain_offset / (CHUNK_PLAIN_SIZE as u64)
    }

    /// Returns the byte offset within the chunk for the given plaintext byte offset (v2).
    #[inline]
    pub fn plain_offset_in_chunk(plain_offset: u64) -> usize {
        (plain_offset % (CHUNK_PLAIN_SIZE as u64)) as usize
    }
}

impl Drop for CryptoEngine {
    fn drop(&mut self) {
        self.master_key.zeroize();
        self.k_data.zeroize();
        self.k_hdr.zeroize();
        self.k_name.zeroize();
        self.k_canary.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_encryption_decryption_v2() {
        let engine = CryptoEngine::from_key([0x55; 32]);
        let base_nonce = CryptoEngine::generate_base_nonce();
        let chunk_nonce = CryptoEngine::derive_chunk_nonce(&base_nonce, 0);

        let plaintext = vec![0xAB; CHUNK_PLAIN_SIZE];
        let ciphertext = engine
            .encrypt_chunk(&chunk_nonce, &plaintext)
            .expect("encrypt failed");
        assert_eq!(ciphertext.len(), CHUNK_CIPHER_SIZE);

        let decrypted = engine
            .decrypt_chunk(&chunk_nonce, &ciphertext)
            .expect("decrypt failed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_v3_header_roundtrip() {
        let engine = CryptoEngine::from_key([0x42; 32]);
        let (header, file_id) = engine.create_v3_header();
        assert_eq!(header.len(), HEADER_SIZE_V3);
        assert!(CryptoEngine::is_v3_header(&header));

        let verified_file_id = engine.verify_v3_header(&header).expect("valid header");
        assert_eq!(verified_file_id, file_id);

        // Tamper with magic
        let mut tampered = header;
        tampered[0] ^= 0x01;
        assert!(engine.verify_v3_header(&tampered).is_err());

        // Tamper with tag
        let mut tampered_tag = header;
        tampered_tag[35] ^= 0x01;
        assert!(engine.verify_v3_header(&tampered_tag).is_err());

        // Different engine key cannot verify
        let other_engine = CryptoEngine::from_key([0x43; 32]);
        assert!(other_engine.verify_v3_header(&header).is_err());
    }

    #[test]
    fn test_v3_chunk_encryption_decryption() {
        let engine = CryptoEngine::from_key([0x55; 32]);
        let file_id = [0x99; FILE_ID_SIZE];
        let plaintext = b"Hello, Format v3 Authenticated Encryption!";

        let raw_chunk = engine
            .encrypt_chunk_v3(&file_id, 0, true, plaintext)
            .expect("v3 encrypt failed");
        assert_eq!(raw_chunk.len(), plaintext.len() + CHUNK_OVERHEAD_V3);

        let decrypted = engine
            .decrypt_chunk_v3(&file_id, 0, true, &raw_chunk)
            .expect("v3 decrypt failed");
        assert_eq!(decrypted, plaintext);

        // AAD binding checks: wrong chunk_index fails
        assert!(engine.decrypt_chunk_v3(&file_id, 1, true, &raw_chunk).is_err());

        // Wrong is_last fails
        assert!(engine.decrypt_chunk_v3(&file_id, 0, false, &raw_chunk).is_err());

        // Wrong file_id fails
        let wrong_file_id = [0x88; FILE_ID_SIZE];
        assert!(engine.decrypt_chunk_v3(&wrong_file_id, 0, true, &raw_chunk).is_err());
    }

    #[test]
    fn test_v3_size_calculations() {
        assert_eq!(
            CryptoEngine::calculate_plain_size_v3_checked(HEADER_SIZE_V3 as u64).unwrap(),
            0
        );
        assert!(CryptoEngine::calculate_plain_size_v3_checked(HEADER_SIZE_V3 as u64 - 1).is_err());

        // 1 full chunk: 64 + 65576 = 65640 raw -> 65536 plain
        let one_full_raw = (HEADER_SIZE_V3 + CHUNK_DISK_SIZE_V3) as u64;
        assert_eq!(
            CryptoEngine::calculate_plain_size_v3_checked(one_full_raw).unwrap(),
            CHUNK_PLAIN_SIZE as u64
        );
        assert_eq!(
            CryptoEngine::total_chunks_v3_checked(one_full_raw).unwrap(),
            1
        );

        // Truncated/corrupted remainder: raw size 64 + 40 (only overhead, 0 plain) -> Err
        assert!(CryptoEngine::calculate_plain_size_v3_checked(64 + 40).is_err());
        assert!(CryptoEngine::calculate_plain_size_v3_checked(64 + 10).is_err());

        // Partial chunk: 1000 plain bytes -> raw = 64 + 1000 + 40 = 1104
        let partial_raw = (HEADER_SIZE_V3 + 1000 + CHUNK_OVERHEAD_V3) as u64;
        assert_eq!(
            CryptoEngine::calculate_plain_size_v3_checked(partial_raw).unwrap(),
            1000
        );
        assert_eq!(
            CryptoEngine::total_chunks_v3_checked(partial_raw).unwrap(),
            1
        );
    }

    #[test]
    fn test_argon2id_derivation() {
        let salt = [0x12; 16];
        let key1 = KeyDerivation::derive_key("secure_password", &salt).expect("kdf failed");
        let key2 = KeyDerivation::derive_key("secure_password", &salt).expect("kdf failed");
        assert_eq!(key1, key2);

        let diff_salt = [0x13; 16];
        let key3 = KeyDerivation::derive_key("secure_password", &diff_salt).expect("kdf failed");
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_derive_key_zeroizing() {
        let salt = [0x55; 16];
        let key = KeyDerivation::derive_key("password123", &salt).expect("kdf failed");
        let _: zeroize::Zeroizing<[u8; KEY_SIZE]> = key;
        assert_ne!(*key, [0u8; KEY_SIZE]);
    }

    #[test]
    fn test_vault_verification_token() {
        let key = [0x42u8; KEY_SIZE];
        let wrong_key = [0x43u8; KEY_SIZE];
        let engine = CryptoEngine::from_key(key);
        let wrong_engine = CryptoEngine::from_key(wrong_key);

        let token = engine
            .generate_vault_verification_token()
            .expect("token generation failed");
        assert!(engine.verify_vault_token(&token));
        assert!(!wrong_engine.verify_vault_token(&token));
        assert!(!engine.verify_vault_token("invalid_hex_string"));

        // Test v2 backward compatibility: manually encrypt v2 canary with master_key
        const CANARY_NONCE: [u8; NONCE_SIZE] = *b"mycloud-canary-nonce-24b";
        const CANARY_PLAINTEXT: &[u8] = b"mycloud-vault-verified-v1";
        let cipher_v2 = XSalsa20Poly1305::new(SalsaKey::from_slice(&key));
        let nonce_v2 = SalsaNonce::from_slice(&CANARY_NONCE);
        let v2_encrypted = SalsaAead::encrypt(&cipher_v2, nonce_v2, CANARY_PLAINTEXT).unwrap();
        let v2_token = data_encoding::HEXLOWER.encode(&v2_encrypted);
        assert!(engine.verify_vault_token(&v2_token));
    }

    #[test]
    fn test_key_wrapping_and_recovery_roundtrip() {
        let dmk = KeyDerivation::generate_master_key();
        let salt = KeyDerivation::generate_salt();

        let kek1 = KeyDerivation::derive_key("secure_pass_1", &salt).expect("kdf failed");
        let wrapped = KeyDerivation::wrap_key(&kek1, &dmk).expect("wrap failed");
        assert_eq!(wrapped.len(), 144);

        let unwrapped = KeyDerivation::unwrap_key(&kek1, &wrapped).expect("unwrap failed");
        assert_eq!(*unwrapped, *dmk);

        let kek2 = KeyDerivation::derive_key("wrong_pass_2", &salt).expect("kdf failed");
        let bad_unwrap = KeyDerivation::unwrap_key(&kek2, &wrapped);
        assert!(matches!(bad_unwrap, Err(CryptoError::DecryptionFailed)));

        let recovery_key = KeyDerivation::generate_recovery_key();
        assert_eq!(recovery_key.len(), 52);
        let rec_kek = KeyDerivation::derive_kek_from_recovery(&recovery_key).expect("rec kdf failed");
        let wrapped_rec = KeyDerivation::wrap_key(&rec_kek, &dmk).expect("wrap rec failed");

        let unwrapped_rec = KeyDerivation::unwrap_key(&rec_kek, &wrapped_rec).expect("unwrap rec failed");
        assert_eq!(*unwrapped_rec, *dmk);

        // Test with dash and space formatting
        let formatted_rec_key = format!(
            "{}-{}-{} {}",
            &recovery_key[0..10],
            &recovery_key[10..20],
            &recovery_key[20..35],
            &recovery_key[35..52]
        );
        let rec_kek_formatted = KeyDerivation::derive_kek_from_recovery(&formatted_rec_key)
            .expect("rec kdf formatted failed");
        let unwrapped_rec2 = KeyDerivation::unwrap_key(&rec_kek_formatted, &wrapped_rec)
            .expect("unwrap formatted rec failed");
        assert_eq!(*unwrapped_rec2, *dmk);

        // Tampered wrapped ciphertext should fail
        let mut tampered = wrapped.clone();
        tampered.replace_range(50..52, "00");
        if tampered == wrapped {
            tampered.replace_range(50..52, "ff");
        }
        assert!(KeyDerivation::unwrap_key(&kek1, &tampered).is_err());

        // Invalid length wrapped string should fail
        assert!(KeyDerivation::unwrap_key(&kek1, "deadbeef").is_err());
    }
}

