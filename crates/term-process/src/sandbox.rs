//! The credential files a sandbox keeps unreadable.

/// The files below the home folder that hold credentials.
pub const CREDENTIAL_FILES: &[&str] = &[
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".m2/settings.xml",
    ".m2/settings-security.xml",
    ".gradle/gradle.properties",
    ".npmrc",
    ".netrc",
];
