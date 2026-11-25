//! The permission modes and effort levels a session can switch to. The CLI may know more than this build, so a
//! name it is not listed here still passes through (R7).

/// How the CLI asks before it acts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PermissionMode {
    /// Asks for everything the rules do not allow.
    Default,
    /// Takes edits without asking.
    AcceptEdits,
    /// Only plans; changes nothing.
    Plan,
    /// Lets a classifier decide what needs asking.
    Auto,
    /// Asks for nothing; the CLI takes it only when the launch allowed it.
    BypassPermissions,
    /// Refuses what the rules do not allow, without asking.
    DontAsk,
    /// A mode of a newer CLI, by its name on the wire.
    Other(String),
}

impl PermissionMode {
    /// The mode's name on the wire, such as `acceptEdits`.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::Auto => "auto",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Other(name) => name,
        }
    }
}

impl From<&str> for PermissionMode {
    fn from(name: &str) -> PermissionMode {
        match name {
            "default" => PermissionMode::Default,
            "acceptEdits" => PermissionMode::AcceptEdits,
            "plan" => PermissionMode::Plan,
            "auto" => PermissionMode::Auto,
            "bypassPermissions" => PermissionMode::BypassPermissions,
            "dontAsk" => PermissionMode::DontAsk,
            other => PermissionMode::Other(other.to_string()),
        }
    }
}

/// How hard the model thinks before it answers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effort {
    /// Hardly at all.
    Minimal,
    /// Little.
    Low,
    /// Some.
    Medium,
    /// Much.
    High,
    /// More than much.
    XHigh,
    /// As much as the model can.
    Max,
    /// A level of a newer CLI or model, by its name on the wire.
    Other(String),
}

impl Effort {
    /// The level's name on the wire, such as `xhigh`.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
            Effort::Other(name) => name,
        }
    }
}

impl From<&str> for Effort {
    fn from(name: &str) -> Effort {
        match name {
            "minimal" => Effort::Minimal,
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "xhigh" => Effort::XHigh,
            "max" => Effort::Max,
            other => Effort::Other(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_comes_back_as_it_went_in() {
        for name in [
            "default",
            "acceptEdits",
            "plan",
            "auto",
            "bypassPermissions",
            "dontAsk",
            "future",
        ] {
            assert_eq!(PermissionMode::from(name).as_str(), name);
        }
        for name in ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"] {
            assert_eq!(Effort::from(name).as_str(), name);
        }
        assert_eq!(PermissionMode::from("plan"), PermissionMode::Plan);
        assert_eq!(Effort::from("ultra"), Effort::Other("ultra".into()));
    }
}
