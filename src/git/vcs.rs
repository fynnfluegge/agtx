use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Version-control backend selected explicitly by project configuration.
///
/// There is deliberately no `Auto` variant. An omitted project setting means
/// Git; repositories that want Jujutsu opt in with `vcs = "jj"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VcsKind {
    #[default]
    Git,
    Jj,
}

impl VcsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Jj => "jj",
        }
    }
}

impl fmt::Display for VcsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for VcsKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "git" => Ok(Self::Git),
            "jj" => Ok(Self::Jj),
            other => Err(format!("unsupported VCS '{other}'; expected 'git' or 'jj'")),
        }
    }
}
