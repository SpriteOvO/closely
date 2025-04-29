use std::{
    fmt::Display,
    ops,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Deserialize;
use spdlog::prelude::*;

pub trait Validator {
    fn validate(&self) -> anyhow::Result<()>;
}

impl<T: Validator> Validator for Option<T> {
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(data) = self {
            data.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Accessor<T> {
    #[serde(skip)]
    is_validated: AtomicBool,
    #[serde(flatten)]
    data: T,
}

impl<T: Clone> Clone for Accessor<T> {
    fn clone(&self) -> Self {
        Self {
            is_validated: AtomicBool::new(self.is_validated()),
            data: self.data.clone(),
        }
    }
}

impl<T: PartialEq> PartialEq for Accessor<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data.eq(&other.data)
    }
}

impl<T: Display> Display for Accessor<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.data.fmt(f)
    }
}

impl<T> Accessor<T> {
    pub fn new(data: T) -> Self {
        Self {
            is_validated: AtomicBool::new(false),
            data,
        }
    }

    pub fn is_validated(&self) -> bool {
        self.is_validated.load(Ordering::Relaxed)
    }

    fn ensure_validated(&self) {
        if !self.is_validated() {
            panic!("config accessed before validation");
        }
    }

    pub fn into_inner(self) -> T {
        self.ensure_validated();
        self.data
    }
}

impl<T: Validator> Accessor<T> {
    pub fn new_then_validate(data: T) -> anyhow::Result<Self> {
        let accessor = Self::new(data);
        accessor.validate().map(|_| accessor)
    }
}

impl<T: Validator> Validator for Accessor<T> {
    fn validate(&self) -> anyhow::Result<()> {
        if !self.is_validated() {
            self.data.validate()?;
            self.is_validated.store(true, Ordering::Relaxed);
        } else {
            trace!("config validated multiple times");
        }
        Ok(())
    }
}

impl<T: Validator> ops::Deref for Accessor<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.ensure_validated();
        &self.data
    }
}

impl<T: Validator> ops::DerefMut for Accessor<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ensure_validated();
        &mut self.data
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    #[derive(Clone, Copy)]
    struct Odd(u32);

    impl Validator for Odd {
        fn validate(&self) -> anyhow::Result<()> {
            if self.0 % 2 == 0 {
                Err(anyhow!("{} is not odd", self.0))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn validation() {
        let right = Accessor::new(Odd(41));
        let wrong = Accessor::new(Odd(42));

        assert!(right.validate().is_ok());
        _ = *right;

        assert!(wrong.validate().is_err());
    }

    #[test]
    #[should_panic(expected = "config accessed before validation")]
    fn panic_if_accessed_before_validation() {
        let accessor = Accessor::new(Odd(41));
        _ = *accessor;
    }

    #[test]
    #[should_panic(expected = "config accessed before validation")]
    fn panic_if_accessed_invalid() {
        let accessor = Accessor::new(Odd(42));
        assert!(accessor.validate().is_err());
        _ = *accessor;
    }
}
