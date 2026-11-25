//! Files encrypted on disk (E17), the restored state and the editor's recovery copies: AES-256 in counter mode and
//! HMAC-SHA-256 over the whole file, both from CommonCrypto, with a random key kept in the keychain under the bundle
//! identifier as its service (E19).

use std::ffi::c_void;
use std::ptr;

pub mod former;
mod keychain;
pub mod secrets;

pub use keychain::key;

/// What a sealed file starts with, and the version of its layout.
const MAGIC: &[u8; 4] = b"TRS1";
const IV_LEN: usize = 16;
const TAG_LEN: usize = 32;
/// The length of the key, of which the keys for encrypting and for the tag are derived.
pub const KEY_LEN: usize = 32;

const K_CC_ENCRYPT: u32 = 0;
const K_CC_MODE_CTR: u32 = 4;
const K_CC_ALGORITHM_AES: u32 = 0;
const CC_NO_PADDING: u32 = 0;
const K_CC_MODE_OPTION_CTR_BE: u32 = 2;
const K_CC_HMAC_ALG_SHA256: u32 = 2;
const K_CC_SUCCESS: i32 = 0;

// The library is part of libSystem, which every program links.
unsafe extern "C" {
    fn CCCryptorCreateWithMode(
        op: u32,
        mode: u32,
        alg: u32,
        padding: u32,
        iv: *const c_void,
        key: *const c_void,
        key_length: usize,
        tweak: *const c_void,
        tweak_length: usize,
        num_rounds: i32,
        options: u32,
        cryptor: *mut *mut c_void,
    ) -> i32;
    fn CCCryptorUpdate(
        cryptor: *mut c_void,
        data_in: *const c_void,
        data_in_length: usize,
        data_out: *mut c_void,
        data_out_available: usize,
        data_out_moved: *mut usize,
    ) -> i32;
    fn CCCryptorRelease(cryptor: *mut c_void) -> i32;
    fn CCHmac(
        algorithm: u32,
        key: *const c_void,
        key_length: usize,
        data: *const c_void,
        data_length: usize,
        mac_out: *mut c_void,
    );
    fn CCRandomGenerateBytes(bytes: *mut c_void, count: usize) -> i32;
}

/// Why a sealed file could not be opened or made.
#[derive(Debug, PartialEq, Eq)]
pub enum SealError {
    /// The file is not one this build sealed, or was changed after.
    Damaged,
    /// CommonCrypto or the keychain refused, with its status.
    System(&'static str, i32),
}

impl std::error::Error for SealError {}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealError::Damaged => write!(f, "the sealed file is damaged or sealed with another key"),
            SealError::System(what, status) => write!(f, "{what} failed with status {status}"),
        }
    }
}

/// `count` random bytes from the system's generator.
fn random(count: usize) -> Result<Vec<u8>, SealError> {
    let mut bytes = vec![0u8; count];
    // SAFETY: The buffer holds `count` writable bytes.
    let status = unsafe { CCRandomGenerateBytes(bytes.as_mut_ptr().cast(), count) };
    if status != K_CC_SUCCESS {
        return Err(SealError::System("CCRandomGenerateBytes", status));
    }
    Ok(bytes)
}

/// A new random key, as the keychain keeps one and a self-test keeps its own.
pub fn new_key() -> Result<[u8; KEY_LEN], SealError> {
    random(KEY_LEN)?.try_into().map_err(|_| SealError::Damaged)
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; TAG_LEN] {
    let mut out = [0u8; TAG_LEN];
    // SAFETY: Key and data are read for their lengths; the output holds the 32 bytes of a SHA-256 tag.
    unsafe {
        CCHmac(
            K_CC_HMAC_ALG_SHA256,
            key.as_ptr().cast(),
            key.len(),
            data.as_ptr().cast(),
            data.len(),
            out.as_mut_ptr().cast(),
        );
    }
    out
}

/// The keys for encrypting and for the tag, each derived from `key` for its one use; the labels predate the recovery
/// copies and stay, so files sealed before still open.
fn derived(key: &[u8; KEY_LEN]) -> ([u8; TAG_LEN], [u8; TAG_LEN]) {
    (hmac(key, b"restore encryption"), hmac(key, b"restore tag"))
}

/// `data` run through AES-256-CTR from `iv`, which encrypts and decrypts alike.
fn ctr(key: &[u8; TAG_LEN], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, SealError> {
    let mut cryptor: *mut c_void = ptr::null_mut();
    // SAFETY: Key and IV are read for their lengths, no tweak is passed, and `cryptor` receives the new object.
    let status = unsafe {
        CCCryptorCreateWithMode(
            K_CC_ENCRYPT,
            K_CC_MODE_CTR,
            K_CC_ALGORITHM_AES,
            CC_NO_PADDING,
            iv.as_ptr().cast(),
            key.as_ptr().cast(),
            key.len(),
            ptr::null(),
            0,
            0,
            K_CC_MODE_OPTION_CTR_BE,
            &raw mut cryptor,
        )
    };
    if status != K_CC_SUCCESS {
        return Err(SealError::System("CCCryptorCreateWithMode", status));
    }
    let mut out = vec![0u8; data.len()];
    let mut moved = 0usize;
    // SAFETY: `cryptor` was made above; a stream mode writes as many bytes as it reads, for which `out` has room.
    let status = unsafe {
        CCCryptorUpdate(
            cryptor,
            data.as_ptr().cast(),
            data.len(),
            out.as_mut_ptr().cast(),
            out.len(),
            &raw mut moved,
        )
    };
    // SAFETY: Released once, after its last use.
    unsafe { CCCryptorRelease(cryptor) };
    if status != K_CC_SUCCESS || moved != data.len() {
        return Err(SealError::System("CCCryptorUpdate", status));
    }
    Ok(out)
}

/// `plain` encrypted with `key` and tagged: magic, IV, cipher text, tag.
pub fn seal(key: &[u8; KEY_LEN], plain: &[u8]) -> Result<Vec<u8>, SealError> {
    let (encryption, tag_key) = derived(key);
    let iv = random(IV_LEN)?;
    let mut out = Vec::with_capacity(MAGIC.len() + IV_LEN + plain.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ctr(&encryption, &iv, plain)?);
    let tag = hmac(&tag_key, &out);
    out.extend_from_slice(&tag);
    Ok(out)
}

/// What `seal` made of `sealed` with `key`, after its tag checks out.
pub fn open(key: &[u8; KEY_LEN], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
    if sealed.len() < MAGIC.len() + IV_LEN + TAG_LEN || !sealed.starts_with(MAGIC) {
        return Err(SealError::Damaged);
    }
    let (encryption, tag_key) = derived(key);
    let (body, tag) = sealed.split_at(sealed.len() - TAG_LEN);
    let expected = hmac(&tag_key, body);
    // Compared without stopping at the first difference, so the time taken tells nothing about the tag.
    let differs = expected.iter().zip(tag).fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if differs != 0 {
        return Err(SealError::Damaged);
    }
    let iv = &body[MAGIC.len()..MAGIC.len() + IV_LEN];
    ctr(&encryption, iv, &body[MAGIC.len() + IV_LEN..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_sealed_opens_with_its_key_only() {
        let key = [7u8; KEY_LEN];
        let plain = b"{\"windows\":[]} and some more text to cross a block".to_vec();
        let sealed = seal(&key, &plain).unwrap();
        assert!(!sealed.windows(plain.len()).any(|part| part == plain.as_slice()));
        assert_eq!(open(&key, &sealed).unwrap(), plain);
        assert_eq!(open(&[8u8; KEY_LEN], &sealed), Err(SealError::Damaged));
    }

    #[test]
    fn a_changed_or_short_file_does_not_open() {
        let key = [1u8; KEY_LEN];
        let mut sealed = seal(&key, b"hello").unwrap();
        let middle = sealed.len() / 2;
        sealed[middle] ^= 1;
        assert_eq!(open(&key, &sealed), Err(SealError::Damaged));
        assert_eq!(open(&key, b"TRS1"), Err(SealError::Damaged));
        assert_eq!(open(&key, &[0u8; 80]), Err(SealError::Damaged));
    }

    #[test]
    fn two_seals_of_the_same_text_differ() {
        let key = [3u8; KEY_LEN];
        assert_ne!(seal(&key, b"same").unwrap(), seal(&key, b"same").unwrap());
    }

    #[test]
    fn counter_mode_matches_the_published_vector() {
        // NIST SP 800-38A, F.5.5, CTR-AES256.Encrypt, first block.
        let key: [u8; 32] = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
            .try_into()
            .unwrap();
        let iv = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plain = hex("6bc1bee22e409f96e93d7e117393172a");
        assert_eq!(ctr(&key, &iv, &plain).unwrap(), hex("601ec313775789a5b7a7f504bbf3d228"));
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }
}
