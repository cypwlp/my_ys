//! Streaming hashes, so a 40 GB archive is checked without loading it.

use crate::engine::EngineError;
use digest::{Digest, DynDigest};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Sha256,
    Md5,
}

impl Algo {
    pub fn label(self) -> &'static str {
        match self {
            Algo::Sha256 => "SHA-256",
            Algo::Md5 => "MD5",
        }
    }

    fn digester(self) -> Box<dyn DynDigest> {
        match self {
            Algo::Sha256 => Box::new(sha2::Sha256::new()),
            Algo::Md5 => Box::new(md5::Md5::new()),
        }
    }
}


/// One pass, 64 KiB buffer, lower-case hex.
pub fn digest_file(path: &Path, algo: Algo) -> Result<String, EngineError> {
    let mut file = std::fs::File::open(path).map_err(|e| EngineError::Io(e.to_string()))?;
    let mut digester = algo.digester();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| EngineError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        digester.update(&buf[..n]);
    }
    Ok(digester
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
