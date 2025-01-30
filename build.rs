use shadow_rs::{BuildPattern::RealTime, ShadowBuilder};

fn main() {
    ShadowBuilder::builder()
        .build_pattern(RealTime)
        .build()
        .unwrap();
}
