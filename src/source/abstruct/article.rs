use std::fmt;

use chrono::{DateTime, Local};

use super::User;

#[derive(Clone, Debug, PartialEq)]
pub struct Article {
    pub unique_id: String,
    pub title: String,
    pub body: String,
    pub link: String,
    pub author: User,
    pub tags: Vec<String>,
    pub created_time: DateTime<Local>,
    // e.g. for GitHub, possible values are "Issue", "PR"
    pub kind: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Articles(pub Vec<Article>);

impl fmt::Display for Articles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| &p.link).collect::<Vec<_>>()
        )
    }
}

#[derive(Clone, Debug)]
pub struct ArticlesRef<'a>(pub(crate) Vec<&'a Article>);

impl fmt::Display for ArticlesRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}",
            self.0.iter().map(|p| &p.link).collect::<Vec<_>>()
        )
    }
}
