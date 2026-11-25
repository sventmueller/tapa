//! Profiles (F161): named sets of font, theme, shell, environment and folder, each inheriting what it leaves out from
//! another profile and in the end from the settings themselves.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::schema::{Config, Font, ThemeChoice};

/// A profile as the file keeps it; what it leaves out comes from `inherits` or the settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Its name.
    pub name: String,
    /// The profile it takes what it leaves out from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits: Option<String>,
    /// The face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<Font>,
    /// The colours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeChoice>,
    /// The program instead of the login shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Variables set on top of the login shell's environment.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The folder new sessions start in, unless they inherit one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
}

/// Why a profile could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// No profile has this name.
    Missing(String),
    /// The profiles inherit from one another in a circle.
    Circle(Vec<String>),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Missing(name) => write!(f, "no profile `{name}`"),
            ProfileError::Circle(names) => write!(f, "profiles inherit in a circle: {}", names.join(" → ")),
        }
    }
}

impl Config {
    /// The profile `name` with everything it inherits filled in; the name `Default` without a profile of that name
    /// is the settings themselves.
    pub fn profile(&self, name: &str) -> Result<Profile, ProfileError> {
        let mut chain = Vec::new();
        let mut next = Some(name.to_string());
        while let Some(current) = next {
            if chain.iter().any(|p: &&Profile| p.name == current) {
                let mut names: Vec<String> = chain.iter().map(|p| p.name.clone()).collect();
                names.push(current);
                return Err(ProfileError::Circle(names));
            }
            match self.profiles.iter().find(|p| p.name == current) {
                Some(profile) => {
                    next = profile.inherits.clone();
                    chain.push(profile);
                }
                None if current == "Default" || !chain.is_empty() && current == self.default_profile => next = None,
                None => return Err(ProfileError::Missing(current)),
            }
        }
        let mut resolved = Profile {
            name: name.to_string(),
            font: Some(self.font.clone()),
            theme: Some(self.theme.clone()),
            ..Profile::default()
        };
        for profile in chain.iter().rev() {
            if profile.font.is_some() {
                resolved.font.clone_from(&profile.font);
            }
            if profile.theme.is_some() {
                resolved.theme.clone_from(&profile.theme);
            }
            if profile.command.is_some() {
                resolved.command.clone_from(&profile.command);
            }
            if profile.directory.is_some() {
                resolved.directory.clone_from(&profile.directory);
            }
            resolved
                .env
                .extend(profile.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        Ok(resolved)
    }

    /// The face of the default profile, the settings' own where it names none or cannot be resolved.
    pub fn face(&self) -> Font {
        self.profile(&self.default_profile)
            .ok()
            .and_then(|profile| profile.font)
            .unwrap_or_else(|| self.font.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let (config, warnings) = Config::parse(
            r#"
            [[profile]]
            name = "Work"
            directory = "/work"
            env = { STAGE = "dev", A = "1" }
            [profile.font]
            size = 15

            [[profile]]
            name = "Prod"
            inherits = "Work"
            env = { STAGE = "prod" }
            [profile.theme]
            dark = "Red Alert"
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        config
    }

    #[test]
    fn a_profile_takes_what_it_leaves_out_from_the_one_it_inherits() {
        let prod = config().profile("Prod").unwrap();
        assert_eq!(prod.directory, Some(PathBuf::from("/work")));
        assert_eq!(prod.env["STAGE"], "prod");
        assert_eq!(prod.env["A"], "1");
        assert_eq!(prod.font.unwrap().size, 15.0);
        assert_eq!(prod.theme.unwrap().dark, "Red Alert");
    }

    #[test]
    fn the_default_profile_is_the_settings() {
        let config = config();
        let default = config.profile("Default").unwrap();
        assert_eq!(default.font, Some(config.font.clone()));
        assert_eq!(default.command, None);
    }

    #[test]
    fn features_and_axes_are_read_from_their_short_forms() {
        let (config, warnings) = Config::parse(
            "version = 1\n[font]\nligatures = false\nfeatures = [\"ss01\", \"-calt\", \"cv05=2\", \"toolong\", \"x=y\"]\n\
             variations = { wght = 450, bad = 1 }\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            config.font.feature_settings(),
            vec![
                ("ss01".into(), 1),
                ("calt".into(), 0),
                ("cv05".into(), 2),
                ("liga".into(), 0)
            ]
        );
        assert_eq!(config.font.variation_settings(), vec![(0x7767_6874, 450.0)]);
    }

    #[test]
    fn the_face_is_the_default_profiles() {
        let mut config = config();
        assert_eq!(config.face(), config.font);
        config.default_profile = "Prod".into();
        assert_eq!(config.face().size, 15.0);
    }

    #[test]
    fn missing_and_circular_profiles_are_errors() {
        let mut config = config();
        assert_eq!(config.profile("Nope"), Err(ProfileError::Missing("Nope".into())));
        config.profiles[0].inherits = Some("Prod".into());
        assert!(matches!(config.profile("Prod"), Err(ProfileError::Circle(_))));
    }
}
