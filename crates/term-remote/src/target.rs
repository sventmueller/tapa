//! Where a remote workspace works: a host as ssh takes it, the user and port, and the folder there.

use serde::{Deserialize, Serialize};

/// The machine and folder of a remote workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Target {
    /// A name from `~/.ssh/config` or an address.
    pub host: String,
    /// The port, where it is not ssh's own or the configuration's.
    #[serde(default)]
    pub port: Option<u16>,
    /// The user, where it is not the configuration's or the local one.
    #[serde(default)]
    pub user: Option<String>,
    /// The folder there; empty for the home folder.
    #[serde(default)]
    pub folder: String,
}

impl Target {
    /// Whether ssh can take the host and user as they are: neither starts with a dash, which ssh would read as an
    /// option, nor holds white space, `@` beyond the user's or control characters.
    pub fn is_valid(&self) -> bool {
        let word = |text: &str| {
            !text.is_empty() && !text.starts_with('-') && !text.chars().any(|c| c.is_whitespace() || c.is_control())
        };
        word(&self.host)
            && !self.host.contains('@')
            && self
                .user
                .as_deref()
                .is_none_or(|user| user.is_empty() || (word(user) && !user.contains('@')))
    }

    /// `user@host`, or the host alone.
    pub fn destination(&self) -> String {
        match self.user.as_deref().filter(|user| !user.is_empty()) {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// What names the target in the keychain and the interface: `user@host`, with `:port` where one is set.
    pub fn account(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.destination()),
            None => self.destination(),
        }
    }

    /// The last part of the folder, or the host for the home folder, as a workspace is named by default.
    pub fn folder_name(&self) -> String {
        self.folder
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty() && *name != "~")
            .map_or_else(|| self.host.clone(), str::to_string)
    }

    /// The port option, where one is set.
    pub(crate) fn port_args(&self) -> Vec<String> {
        self.port
            .map(|port| vec!["-p".to_string(), port.to_string()])
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(host: &str, user: Option<&str>, port: Option<u16>) -> Target {
        Target {
            host: host.into(),
            port,
            user: user.map(Into::into),
            folder: "/srv/app/".into(),
        }
    }

    #[test]
    fn the_destination_and_account_name_user_host_and_port() {
        assert_eq!(target("box", Some("sven"), None).destination(), "sven@box");
        assert_eq!(target("box", None, Some(2222)).account(), "box:2222");
        assert_eq!(target("box", Some(""), None).destination(), "box");
        assert_eq!(target("box", None, None).folder_name(), "app");
        let home = Target {
            folder: String::new(),
            ..target("box", None, None)
        };
        assert_eq!(home.folder_name(), "box");
    }

    #[test]
    fn a_host_or_user_ssh_would_read_as_an_option_is_refused() {
        assert!(target("box.example.com", Some("sven"), None).is_valid());
        assert!(!target("-oProxyCommand=x", None, None).is_valid());
        assert!(!target("box", Some("-l"), None).is_valid());
        assert!(!target("two words", None, None).is_valid());
        assert!(!target("a@b", None, None).is_valid());
        assert!(!target("", None, None).is_valid());
    }
}
