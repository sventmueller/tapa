//! The one way onto the network: HTTPS checked against the system's certificate store, a [`Net`] that tests stand in
//! for, and downloads that hash what arrives while reporting progress and listening for [`Cancel`]. Installing agents
//! (`term_acp::install`) and fetching the ACP registry go through it, and only on the user's word (E21).
//!
//! ```
//! use std::io;
//!
//! use term_net::{Cancel, Download, Net};
//!
//! struct Canned;
//!
//! impl Net for Canned {
//!     fn get(&self, _url: &str, _accept: &str) -> io::Result<Download> {
//!         Ok(Download { length: Some(5), body: Box::new(io::Cursor::new(b"hello".to_vec())) })
//!     }
//! }
//!
//! let path = std::env::temp_dir().join(format!("term-net-doc-{}", std::process::id()));
//! let mut seen = Vec::new();
//! let digest = term_net::download(&Canned, "https://example.com/x", &path, &Cancel::new(), None, &mut |done, _| {
//!     seen.push(done)
//! })?;
//! assert_eq!(digest, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
//! assert_eq!(seen, [5]);
//! # std::fs::remove_file(&path)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

mod download;
mod net;

pub use download::{DownloadError, download};
pub use net::{Download, Http, Net, Offline};
pub use term_base::Cancel;
