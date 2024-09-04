use std::borrow::Cow;

use rand::distributions::{Alphanumeric, DistString};

pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
}

pub const PACKAGE: Package = Package {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
};

pub fn user_agent(dynamic: bool) -> String {
    format!(
        "{}/{} (FAIR USE, PLEASE DO NOT BLOCK. Source opened on github.com/SpriteOvO/{}{})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_NAME"),
        if dynamic {
            Cow::Owned(format!(
                ". {}",
                Alphanumeric.sample_string(&mut rand::thread_rng(), 8)
            ))
        } else {
            Cow::Borrowed("")
        }
    )
}
