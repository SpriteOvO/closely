use std::{borrow::Cow, env, error::Error as StdError, str::FromStr};

pub trait AsSecretRef<'a, T = &'a str> {
    fn as_secret_ref(&'a self) -> SecretRef<'a, T>;
}

pub enum SecretRef<'a, T = &'a str> {
    Lit(T),
    Env(&'a str),
}

impl<'a> SecretRef<'a, &'a str> {
    pub fn get_str(&self) -> anyhow::Result<Cow<'a, str>> {
        match self {
            Self::Lit(lit) => Ok(Cow::Borrowed(lit)),
            Self::Env(key) => Ok(Cow::Owned(env::var(key)?)),
        }
    }
}

impl<T: Copy + FromStr> SecretRef<'_, T>
where
    <T as FromStr>::Err: StdError + Send + Sync + 'static,
{
    pub fn get_parse_copy(&self) -> anyhow::Result<T> {
        match self {
            Self::Lit(lit) => Ok(*lit),
            Self::Env(key) => Ok(env::var(key)?.parse()?),
        }
    }
}

impl<T: ToOwned> SecretRef<'_, T>
where
    <T as ToOwned>::Owned: FromStr,
    <<T as ToOwned>::Owned as FromStr>::Err: StdError + Send + Sync + 'static,
{
    pub fn get_parse_cow(&self) -> anyhow::Result<Cow<T>> {
        match self {
            Self::Lit(lit) => Ok(Cow::Borrowed(lit)),
            Self::Env(key) => Ok(Cow::Owned(env::var(key)?.parse()?)),
        }
    }
}

#[macro_export]
macro_rules! secret_enum {
    ( $($(#[$attr:meta])* $vis:vis enum $name:ident { $field:ident($type:ident)$(,)? })+ ) => {
        $(
            paste::paste! {
                mod secret_enum_private {
                    use super::*;

                    $(#[$attr])* pub enum $name {
                        $field($type),
                        [<$field Env>](String),
                    }

                    impl $crate::config::Validator for $name {
                        fn validate(&self) -> anyhow::Result<()> {
                            match self {
                                Self::$field(_) => Ok(()),
                                Self::[<$field Env>](key) => match std::env::var(key) {
                                    Ok(_) => Ok(()),
                                    Err(err) => bail!("{err} ({key})"),
                                },
                            }
                        }
                    }
                }

                $(#[$attr])* $vis struct $name($crate::config::Accessor<secret_enum_private::$name>);

                impl $name {
                    pub fn with_env(key: impl Into<String>) -> Self {
                        paste::paste! {
                            Self($crate::config::Accessor::new(secret_enum_private::$name::[<$field Env>](key.into())))
                        }
                    }
                }

                impl $crate::config::Validator for $name {
                    fn validate(&self) -> anyhow::Result<()> {
                        self.0.validate()
                    }
                }
            }
            secret_enum!(@IMPL($type) => $name, $field);
        )+
    };
    ( @IMPL(String) => $name:ident, $field:ident ) => {
        impl $name {
            pub fn with_raw(raw: impl Into<String>) -> Self {
                paste::paste! {
                    Self($crate::config::Accessor::new(secret_enum_private::$name::$field(raw.into())))
                }
            }
        }

        impl $crate::config::AsSecretRef<'_> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef {
                paste::paste! {
                    match &*self.0 {
                        secret_enum_private::$name::$field(value) => $crate::config::SecretRef::Lit(value),
                        secret_enum_private::$name::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
                    }
                }
            }
        }
    };
    ( @IMPL($type:ty) => $name:ident, $field:ident ) => {
        impl $name {
            pub fn with_raw(raw: impl Into<$type>) -> Self {
                paste::paste! {
                    Self(secret_enum_private::$name::$field(raw.into()))
                }
            }
        }

        impl $crate::config::AsSecretRef<'_, $type> for $name {
            fn as_secret_ref(&self) -> $crate::config::SecretRef<'_, $type> {
                paste::paste! {
                    match &self.0 {
                        secret_enum_private::$name::$field(value) => $crate::config::SecretRef::Lit(*value),
                        secret_enum_private::$name::[<$field Env>](key) => $crate::config::SecretRef::Env(key),
                    }
                }
            }
        }
    };
}
