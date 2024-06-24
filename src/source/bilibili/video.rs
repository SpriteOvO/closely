use std::{fmt, future::Future, pin::Pin};

use anyhow::{anyhow, ensure};
use serde::Deserialize;
use serde_json as json;

use super::Response;
use crate::{
    platform::{PlatformMetadata, PlatformTrait},
    source::{
        FetcherTrait, Post, PostAttachment, PostAttachmentImage, PostUrl, Posts, Status,
        StatusKind, StatusSource,
    },
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigParams {
    pub user_id: u64,
    pub series_id: u64,
}

impl fmt::Display for ConfigParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "bilibili.video:{},series{}",
            self.user_id, self.series_id
        )
    }
}

mod data {
    use super::*;

    #[derive(Debug, Deserialize)]
    pub struct SeriesArchives {
        pub aids: Vec<u64>,
        pub archives: Vec<Archive>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Archive {
        pub aid: u64,
        pub title: String,
        pub pic: String, // Image URL
        pub bvid: String,
    }
}

pub struct Fetcher {
    params: ConfigParams,
}

impl PlatformTrait for Fetcher {
    fn metadata(&self) -> PlatformMetadata {
        PlatformMetadata {
            display_name: "bilibili 视频",
        }
    }
}

impl FetcherTrait for Fetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for Fetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl Fetcher {
    pub fn new(params: ConfigParams) -> Self {
        Self { params }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let videos = fetch_series_archives(self.params.user_id, self.params.series_id).await?;

        Ok(Status::new(
            StatusKind::Posts(videos),
            StatusSource {
                platform: self.metadata(),
                user: None,
            },
        ))
    }
}

async fn fetch_series_archives(user_id: u64, series_id: u64) -> anyhow::Result<Posts> {
    let resp = reqwest::Client::new()
        .get(format!(
            "https://api.bilibili.com/x/series/archives?mid={user_id}&series_id={series_id}"
        ))
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;

    let status = resp.status();
    ensure!(
        status.is_success(),
        "response status is not success: {resp:?}"
    );

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response: {err}"))?;

    let resp: Response<data::SeriesArchives> = json::from_str(&text)
        .map_err(|err| anyhow!("failed to deserialize response: {err}, text: {text}"))?;
    ensure!(resp.code == 0, "response code is not 0. text: {text}");

    parse_response(resp.data.unwrap())
}

fn parse_response(resp: data::SeriesArchives) -> anyhow::Result<Posts> {
    let videos = resp
        .archives
        .into_iter()
        .map(|archive| Post {
            user: None,
            content: archive.title,
            urls: PostUrl::new_clickable(
                format!("https://www.bilibili.com/video/{}", archive.bvid),
                "查看视频",
            )
            .into(),
            repost_from: None,
            attachments: vec![PostAttachment::Image(PostAttachmentImage {
                media_url: archive.pic,
            })],
        })
        .collect();

    Ok(Posts(videos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deser() {
        let videos = fetch_series_archives(522384919, 3747026).await.unwrap();

        assert!(videos.0.iter().all(|post| !post
            .urls
            .major()
            .as_clickable()
            .unwrap()
            .url
            .is_empty()));
        assert!(videos.0.iter().all(|post| !post.content.is_empty()));
    }
}
