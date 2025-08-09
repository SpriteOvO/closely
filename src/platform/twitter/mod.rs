mod request;
pub(crate) mod source;

use request::TwitterCookies;
use serde::Deserialize;

use crate::{
    config::{Accounts, AsSecretRef, Validator},
    secret_enum,
};

// Global
//

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigGlobal {
    pub account: Accounts<ConfigCookies>,
}

impl Validator for ConfigGlobal {
    fn validate(&self) -> anyhow::Result<()> {
        self.account.validate()?;
        for account in self.account.values() {
            TwitterCookies::new(account.as_secret_ref().get_str()?)?;
        }
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
