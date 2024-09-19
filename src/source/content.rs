#[derive(Clone, Debug, PartialEq)]
pub struct PostContent(Vec<PostContentPart>);

impl PostContent {
    pub fn from_parts(parts: impl IntoIterator<Item = PostContentPart>) -> Self {
        Self(parts.into_iter().collect())
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self(vec![PostContentPart::Plain(text.into())])
    }

    pub fn fallback(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                PostContentPart::Plain(text) => text.as_str(),
                PostContentPart::Link { url, .. } => url.as_str(),
            })
            .collect::<String>()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parts(&self) -> impl Iterator<Item = &PostContentPart> {
        self.0.iter()
    }

    //

    pub fn push_content(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    pub fn with_content(mut self, other: Self) -> Self {
        self.push_content(other);
        self
    }

    pub fn push_plain(&mut self, text: impl Into<String>) {
        self.0.push(PostContentPart::Plain(text.into()));
    }

    pub fn with_plain(mut self, text: impl Into<String>) -> Self {
        self.push_plain(text);
        self
    }

    pub fn push_link(&mut self, display: impl Into<String>, url: impl Into<String>) {
        self.0.push(PostContentPart::Link {
            display: display.into(),
            url: url.into(),
        });
    }

    pub fn with_link(mut self, display: impl Into<String>, url: impl Into<String>) -> Self {
        self.push_link(display, url);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostContentPart {
    Plain(String),
    Link { display: String, url: String },
}
