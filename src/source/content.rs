#[derive(Clone, Debug, PartialEq)]
pub struct PostContent(Vec<PostContentPart>);

impl PostContent {
    pub fn plain(text: impl Into<String>) -> Self {
        Self(vec![PostContentPart::Plain(text.into())])
    }

    pub fn fallback(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                PostContentPart::Plain(text) => text.as_str(),
            })
            .collect::<String>()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parts(&self) -> impl Iterator<Item = &PostContentPart> {
        self.0.iter()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostContentPart {
    Plain(String),
}
