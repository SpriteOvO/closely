use std::{
    collections::{hash_map::Values, HashMap},
    fmt,
};

use anyhow::ensure;
use serde::Deserialize;

use crate::config::{Accessor, ContextualValidator, Validator};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Accounts<T>(HashMap<String, Accessor<T>>);

impl<T: Validator> Validator for Accounts<T> {
    fn validate(&self) -> anyhow::Result<()> {
        for account in self.0.values() {
            account.validate()?;
        }
        Ok(())
    }
}

impl<T> Accounts<T> {
    #[cfg(test)]
    pub fn from_iter(i: impl IntoIterator<Item = (String, Accessor<T>)>) -> Self {
        Self(HashMap::from_iter(i))
    }

    pub fn get(&self, account_ref: &AccountRef) -> &Accessor<T> {
        self.0.get(&account_ref.0.get().0).unwrap()
    }

    pub fn contains(&self, account_ref: &str) -> bool {
        self.0.contains_key(account_ref)
    }

    pub fn values(&self) -> Values<String, Accessor<T>> {
        self.0.values()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AccountRef(Accessor<AccountRefInner>);

impl AccountRef {
    #[cfg(test)]
    pub fn new(name: impl Into<String>) -> Self {
        Self(Accessor::new(AccountRefInner(name.into())))
    }
}

impl<T> ContextualValidator<&Accounts<T>> for AccountRef {
    fn validate(&self, accounts: &Accounts<T>) -> anyhow::Result<()> {
        self.0.validate(accounts)?;
        Ok(())
    }
}

impl fmt::Display for AccountRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct AccountRefInner(String);

impl<T> ContextualValidator<&Accounts<T>> for AccountRefInner {
    fn validate(&self, accounts: &Accounts<T>) -> anyhow::Result<()> {
        ensure!(
            accounts.contains(&self.0),
            "account '{}' is not configured",
            self.0
        );
        Ok(())
    }
}

impl fmt::Display for AccountRefInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
