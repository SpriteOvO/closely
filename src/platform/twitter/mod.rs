mod request;
pub(crate) mod source;

use serde::Deserialize;

use crate::{config::Validator, secret_enum};

// Global
//

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub auth: ConfigCookies,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.auth.validate()?;
        Ok(())
    }
}

secret_enum! {
    #[derive(Clone, Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigCookies {
        Cookies(String),
    }
}
