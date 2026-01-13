pub mod bilibili;
pub mod github;
pub mod qq;
pub mod rss;
pub mod telegram;
pub mod twitter;

pub trait PlatformTraitStatic {
    fn metadata() -> PlatformMetadata;
}

// Do not impl this for your struct, impl `PlatformTraitStatic` instead.
// This trait is designed to be dyn-compatible.
pub trait PlatformTrait: Send + Sync {
    fn metadata(&self) -> PlatformMetadata;
}

impl<T: PlatformTraitStatic + Send + Sync> PlatformTrait for T {
    fn metadata(&self) -> PlatformMetadata {
        T::metadata()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatformMetadata {
    pub display_name: &'static str,
}
