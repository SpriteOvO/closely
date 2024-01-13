pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
}

pub const PACKAGE: Package = Package {
    name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
};
