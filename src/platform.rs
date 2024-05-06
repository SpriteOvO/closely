pub trait PlatformTrait: Send + Sync {
    fn metadata(&self) -> PlatformMetadata;
}

#[derive(Debug)]
pub struct PlatformMetadata {
    pub display_name: &'static str,
}
