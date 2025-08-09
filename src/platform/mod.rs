pub mod bilibili;
pub mod qq;
pub mod telegram;
pub mod twitter;

pub trait PlatformTrait: Send + Sync {
    fn metadata(&self) -> PlatformMetadata;
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatformMetadata {
    pub display_name: &'static str,
}
