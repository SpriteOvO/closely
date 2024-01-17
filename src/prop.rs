use const_format::formatcp;

pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
    pub user_agent: &'static str,
}

pub const PACKAGE: Package = Package {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    user_agent: formatcp!(
        "{}/{}, https://github.com/SpriteOvO/{}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_NAME")
    ),
};
