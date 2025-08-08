use std::borrow::Cow;

use const_format::formatcp;
use rand::distr::{Alphanumeric, SampleString};

pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
}

pub const PACKAGE: Package = Package {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
};

pub enum UserAgent {
    Logo,
    LogoDynamic,
    Mocked,
}

impl UserAgent {
    pub fn as_str(&self) -> Cow<str> {
        match self {
            Self::Logo => Cow::Borrowed(formatcp!(
                "{}/{} (FAIR USE, PLEASE DO NOT BLOCK. Source opened on github.com/SpriteOvO/{})",
                PACKAGE.name,
                PACKAGE.version,
                PACKAGE.name,
            )),
            Self::LogoDynamic => Cow::Owned(format!(
                "{} {})",
                Self::Logo.as_str().strip_suffix(')').unwrap(),
                Alphanumeric.sample_string(&mut rand::rng(), 8)
            )),
            Self::Mocked => Cow::Borrowed(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:136.0) Gecko/20100101 Firefox/136.0",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent() {
        assert_eq!(
            UserAgent::Logo.as_str(),
            format!(
                "closely/{} (FAIR USE, PLEASE DO NOT BLOCK. Source opened on github.com/SpriteOvO/closely)",
                env!("CARGO_PKG_VERSION"),
            )
        );

        let dynamic = UserAgent::LogoDynamic.as_str();
        assert!(dynamic.starts_with(&format!(
            "closely/{} (FAIR USE, PLEASE DO NOT BLOCK. Source opened on github.com/SpriteOvO/closely ",
            env!("CARGO_PKG_VERSION"),
        )));
        assert!(dynamic.ends_with(")"));
        assert_eq!(dynamic.len(), UserAgent::Logo.as_str().len() + 9);
    }
}
