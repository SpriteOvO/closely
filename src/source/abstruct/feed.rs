use std::fmt;

use chrono::{DateTime, Local};

#[derive(Clone, Debug, PartialEq)]
pub struct Feed {
    pub unique_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub link: Option<String>,
    pub pub_date: Option<DateTime<Local>>,
    pub author: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feeds {
    pub title: Option<String>,
    pub items: Vec<Feed>,
}

impl fmt::Display for Feeds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} - {:?}",
            self.title,
            self.items.iter().map(|p| &p.link).collect::<Vec<_>>()
        )
    }
}

#[derive(Clone, Debug)]
pub struct FeedsRef<'a> {
    pub title: Option<&'a str>,
    pub items: Vec<&'a Feed>,
}

impl fmt::Display for FeedsRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} - {:?}",
            self.title,
            self.items.iter().map(|p| &p.link).collect::<Vec<_>>()
        )
    }
}
