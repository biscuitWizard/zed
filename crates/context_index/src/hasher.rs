use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::Path;

const CHUNK_SIZE: usize = 1 << 20; // 1 MiB
const BINARY_SNIFF_SIZE: usize = 4096;
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB

pub type FileHash = [u8; 32];

pub fn is_likely_binary(buf: &[u8]) -> bool {
    let check_len = buf.len().min(BINARY_SNIFF_SIZE);
    let slice = &buf[..check_len];
    slice.iter().any(|&b| b == 0)
}

pub async fn hash_file(fs: &dyn fs::Fs, path: &Path) -> Result<Option<FileHash>> {
    let metadata = fs.metadata(path).await?;
    let meta = match metadata {
        Some(m) => m,
        None => return Ok(None),
    };

    if meta.is_dir || meta.is_symlink {
        return Ok(None);
    }

    if meta.len > MAX_FILE_SIZE {
        return Ok(None);
    }

    let bytes = fs.load_bytes(path).await?;

    if is_likely_binary(&bytes) {
        return Ok(None);
    }

    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(CHUNK_SIZE) {
        hasher.update(chunk);
    }
    let result: [u8; 32] = hasher.finalize().into();
    Ok(Some(result))
}

pub fn hash_bytes(data: &[u8]) -> FileHash {
    let mut hasher = Sha256::new();
    for chunk in data.chunks(CHUNK_SIZE) {
        hasher.update(chunk);
    }
    hasher.finalize().into()
}

pub fn hash_to_hex(hash: &FileHash) -> String {
    hex::encode(hash)
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_bytes_deterministic() {
        let data = b"hello world";
        let h1 = hash_bytes(data);
        let h2 = hash_bytes(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_bytes_different_input() {
        let h1 = hash_bytes(b"hello");
        let h2 = hash_bytes(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_binary_detection() {
        assert!(is_likely_binary(b"hello\x00world"));
        assert!(!is_likely_binary(b"hello world"));
        assert!(!is_likely_binary(b"fn main() { println!(\"hi\"); }"));
    }

    #[test]
    fn test_hash_to_hex() {
        let hash = hash_bytes(b"test");
        let hex = hash_to_hex(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
