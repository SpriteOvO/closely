mod article;
mod feed;
mod file;
mod live;
mod notification;
mod post;
mod status;
mod update;

pub use article::*;
pub use feed::*;
pub use file::*;
pub use live::*;
pub use notification::*;
pub use post::*;
pub use status::*;
pub use update::*;

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub nickname: String,
    pub profile_url: String,
    pub avatar_url: Option<String>,
}
