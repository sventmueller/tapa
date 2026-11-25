//! The client every request goes through, and the `Net` seam installers and the registry fetch with.

use std::io::{self, Read};
use std::time::Duration;

/// An agent for HTTPS only, checked against the system's certificate store, named after the app and `purpose`.
fn client(purpose: &str) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .http_status_as_error(true)
        .user_agent(format!("{} ({purpose})", term_base::identity().name))
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

/// A download in progress.
pub struct Download {
    /// The size, if the server said.
    pub length: Option<u64>,
    /// The bytes.
    pub body: Box<dyn Read + Send>,
}

/// Fetches URLs; the only way installers and the registry reach the network, so tests can stand in for it.
pub trait Net: Send + Sync {
    /// Starts fetching `url`, asking for `accept`; an error for anything but success.
    fn get(&self, url: &str, accept: &str) -> io::Result<Download>;

    /// Fetches `url` as text of at most `limit` bytes.
    fn get_text(&self, url: &str, limit: u64) -> io::Result<String> {
        let mut text = String::new();
        let read = self.get(url, "*/*")?.body.take(limit + 1).read_to_string(&mut text)?;
        if read as u64 > limit {
            return Err(io::Error::other(format!("{url} is larger than {limit} bytes")));
        }
        Ok(text)
    }
}

/// The network over HTTPS only, with time limits for connecting and for each read; a redirect cannot leave HTTPS
/// either.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    /// A client whose requests are for `purpose`, such as `agent register`.
    pub fn new(purpose: &str) -> Http {
        Http { agent: client(purpose) }
    }
}

impl Net for Http {
    fn get(&self, url: &str, accept: &str) -> io::Result<Download> {
        if !url.starts_with("https://") {
            return Err(io::Error::other(format!("{url} is not an HTTPS address")));
        }
        let response = self
            .agent
            .get(url)
            .header("Accept", accept)
            .call()
            .map_err(io::Error::other)?;
        let body = response.into_body();
        Ok(Download {
            length: body.content_length(),
            body: Box::new(body.into_reader()),
        })
    }
}

/// A network that refuses every request, for self-tests and tests that must never leave the Mac.
pub struct Offline;

impl Net for Offline {
    fn get(&self, url: &str, _accept: &str) -> io::Result<Download> {
        Err(io::Error::other(format!("{url}: offline")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Text(&'static str);

    impl Net for Text {
        fn get(&self, _: &str, _: &str) -> io::Result<Download> {
            Ok(Download {
                length: None,
                body: Box::new(io::Cursor::new(self.0.as_bytes())),
            })
        }
    }

    #[test]
    fn text_up_to_the_limit_is_read_and_more_is_refused() {
        assert_eq!(Text("four").get_text("u", 4).unwrap(), "four");
        assert!(Text("fives").get_text("u", 4).is_err());
    }

    #[test]
    fn only_https_leaves_the_mac() {
        let refused = Http::new("test").get("http://example.com/", "*/*").err().unwrap();
        assert!(refused.to_string().contains("not an HTTPS address"));
        assert!(
            Http::new("test").agent.config().https_only(),
            "a redirect cannot leave HTTPS either"
        );
        assert!(Offline.get("https://example.com/", "*/*").is_err());
    }
}
