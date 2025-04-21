use std::{convert::identity, time::Duration};

use anyhow::anyhow;
use humantime_serde::re::humantime;
use reqwest::header::{self, HeaderMap, HeaderValue};

use crate::prop;

pub fn reqwest_client() -> anyhow::Result<reqwest::Client> {
    reqwest_client_with(identity)
}

pub fn reqwest_client_with(
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> anyhow::Result<reqwest::Client> {
    configure(
        reqwest::ClientBuilder::new()
            .timeout(Duration::from_secs(60) * 3)
            .default_headers(HeaderMap::from_iter([(
                header::USER_AGENT,
                HeaderValue::from_str(&prop::UserAgent::Logo.as_str()).unwrap(),
            )])),
    )
    .build()
    .map_err(|err| anyhow!("failed to build reqwest client: {err}"))
}

macro_rules! refl_fn {
    ( $($ty:ident),+ ) => {
        $(paste::paste! {
            pub const fn [<refl_ $ty>]<const V: $ty>() -> $ty {
                V
            }
        })+
    };
}

refl_fn!(bool);

#[macro_export]
macro_rules! serde_impl_default_for {
    ( $struct:ident ) => {
        impl Default for $struct {
            fn default() -> Self {
                // https://stackoverflow.com/a/77858562
                Self::deserialize(serde::de::value::MapDeserializer::<
                    _,
                    serde::de::value::Error,
                >::new(std::iter::empty::<((), ())>()))
                .unwrap()
            }
        }
    };
}

pub fn format_duration_in_sec(dur: Duration) -> String {
    humantime::format_duration(Duration::from_secs(dur.as_secs())).to_string()
}

pub fn format_duration_in_min(dur: Duration) -> String {
    let mins = dur.as_secs() / 60;
    if mins == 0 {
        return "0m".to_string();
    }
    humantime::format_duration(Duration::from_secs(mins * 60)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_in_sec() {
        assert_eq!(format_duration_in_sec(Duration::from_secs(0)), "0s");
        assert_eq!(
            // 2m 3s 456ms
            format_duration_in_sec(Duration::from_millis(123456)),
            "2m 3s"
        );
        assert_eq!(format_duration_in_sec(Duration::from_secs(3)), "3s");
        assert_eq!(format_duration_in_sec(Duration::from_secs(60)), "1m");
        assert_eq!(format_duration_in_sec(Duration::from_secs(80)), "1m 20s");
        assert_eq!(
            format_duration_in_sec(Duration::from_secs(60 * 60 + 80)),
            "1h 1m 20s"
        );
    }

    #[test]
    fn test_format_duration_in_min() {
        assert_eq!(format_duration_in_min(Duration::from_secs(0)), "0m");
        assert_eq!(
            // 2m 3s 456ms
            format_duration_in_min(Duration::from_millis(123456)),
            "2m"
        );
        assert_eq!(format_duration_in_min(Duration::from_secs(3)), "0m");
        assert_eq!(format_duration_in_min(Duration::from_secs(60)), "1m");
        assert_eq!(format_duration_in_min(Duration::from_secs(80)), "1m");
        assert_eq!(
            format_duration_in_min(Duration::from_secs(60 * 60 + 80)),
            "1h 1m"
        );
    }
}
