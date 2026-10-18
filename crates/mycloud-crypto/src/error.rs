use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Argon2 KDF error: {0}")]
    Argon2(String),

    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Decryption failed: MAC tag verification failed or payload corrupted")]
    DecryptionFailed,

    #[error("Invalid nonce: {0}")]
    InvalidNonce(String),

    #[error("Invalid header: {0}")]
    InvalidHeader(String),

    #[error("Base32 decoding error: {0}")]
    Base32Decode(String),

    #[error("Filename decryption error: UTF-8 conversion failed")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
