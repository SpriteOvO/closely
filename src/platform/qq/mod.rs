pub mod notify;
pub(crate) mod onebot11;

use std::fmt;

use serde::Deserialize;

use crate::config::{Accounts, Validator};

// Base
//

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigChat {
    GroupId(u64),
    UserId(u64),
}

impl fmt::Display for ConfigChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GroupId(id) => write!(f, "group={id}"),
            Self::UserId(id) => write!(f, "user={id}"),
        }
    }
}

// Global
//

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub account: Accounts<ConfigAccount>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.account.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigAccount {
    #[serde(alias = "lagrange")]
    pub onebot11: onebot11::ConfigOneBot11,
}

impl Validator for ConfigAccount {
    fn validate(&self) -> anyhow::Result<()> {
        self.onebot11.validate()?;
        Ok(())
    }
}
