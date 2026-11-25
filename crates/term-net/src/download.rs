//! A download into a file, hashed as it arrives.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{Cancel, Net};

/// Why a download stopped.
#[derive(Debug)]
pub enum DownloadError {
    /// It was cancelled.
    Cancelled,
    /// The network failed.
    Network(io::Error),
    /// More arrived than the limit allows.
    TooLarge(u64),
    /// Writing the file failed.
    Io(io::Error),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::Cancelled => f.write_str("the download was cancelled"),
            DownloadError::Network(error) => write!(f, "the download failed: {error}"),
            DownloadError::TooLarge(limit) => write!(f, "the download is larger than {limit} bytes"),
            DownloadError::Io(error) => write!(f, "the download could not be written: {error}"),
        }
    }
}

impl std::error::Error for DownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DownloadError::Network(error) | DownloadError::Io(error) => Some(error),
            DownloadError::Cancelled | DownloadError::TooLarge(_) => None,
        }
    }
}

/// Downloads `url` into `path`, at most `limit` bytes when given, telling `progress` the bytes so far and the size
/// if known; the SHA-256 of what arrived, in lowercase hex.
pub fn download(
    net: &dyn Net,
    url: &str,
    path: &Path,
    cancel: &Cancel,
    limit: Option<u64>,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String, DownloadError> {
    let mut download = net
        .get(url, "application/octet-stream")
        .map_err(DownloadError::Network)?;
    let mut file = File::create(path).map_err(DownloadError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let read = download.body.read(&mut buffer).map_err(DownloadError::Network)?;
        if read == 0 {
            break;
        }
        done += read as u64;
        if let Some(limit) = limit
            && done > limit
        {
            return Err(DownloadError::TooLarge(limit));
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(DownloadError::Io)?;
        progress(done, download.length);
    }
    file.sync_all().map_err(DownloadError::Io)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Download;

    struct Bytes(Vec<u8>);

    impl Net for Bytes {
        fn get(&self, _: &str, _: &str) -> io::Result<Download> {
            Ok(Download {
                length: Some(self.0.len() as u64),
                body: Box::new(io::Cursor::new(self.0.clone())),
            })
        }
    }

    use term_base::testing::scratch;

    #[test]
    fn a_cancelled_download_stops_before_reading() {
        let cancel = Cancel::new();
        cancel.cancel();
        let dir = scratch("net-cancelled");
        let path = dir.join("download");
        let result = download(&Bytes(vec![1; 10]), "u", &path, &cancel, None, &mut |_, _| ());
        assert!(matches!(result, Err(DownloadError::Cancelled)));
    }

    #[test]
    fn more_than_the_limit_is_refused() {
        let dir = scratch("net-limit");
        let path = dir.join("download");
        let result = download(&Bytes(vec![1; 10]), "u", &path, &Cancel::new(), Some(9), &mut |_, _| ());
        assert!(matches!(result, Err(DownloadError::TooLarge(9))));
    }

    #[test]
    fn progress_names_the_size_the_server_gave() {
        let dir = scratch("net-progress");
        let path = dir.join("download");
        let mut seen = Vec::new();
        download(
            &Bytes(vec![1; 10]),
            "u",
            &path,
            &Cancel::new(),
            Some(10),
            &mut |done, total| seen.push((done, total)),
        )
        .unwrap();
        assert_eq!(seen, [(10, Some(10))]);
    }
}
