pub(crate) mod lagrange;
pub mod notify;

use std::{collections::HashMap, fmt};

use serde::Deserialize;

use crate::config::{Accessor, Validator};

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
    pub account: HashMap<String, Accessor<ConfigAccount>>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        for backend in self.account.values() {
            backend.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigAccount {
    pub lagrange: lagrange::ConfigLagrange,
}

impl Validator for ConfigAccount {
    fn validate(&self) -> anyhow::Result<()> {
        self.lagrange.validate()?;
        Ok(())
    }
}
