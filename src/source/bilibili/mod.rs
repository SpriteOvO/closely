pub mod live;
pub mod space;
pub mod video;

use serde::Deserialize;

#[derive(Deserialize)]
struct Response<T> {
    pub(crate) code: i32,
    #[allow(dead_code)]
    pub(crate) message: String,
    pub(crate) data: Option<T>,
}
