use std::{fmt, future::Future, marker::PhantomData, pin::Pin};

use anyhow::{anyhow, bail};
use chrono::NaiveDateTime;
use once_cell::sync::Lazy;
use reqwest::header::{HeaderValue, ACCEPT_LANGUAGE};
use scraper::{Html, Selector};
use spdlog::prelude::*;

use super::{Fetcher, StatusSourceUser};
use crate::{
    config::{Config, PlatformTwitterCom},
    platform::{
        PlatformName, Post, PostAttachment, PostAttachmentImage, Posts, Status, StatusKind,
        StatusSource,
    },
    prop,
};

#[derive(Debug)]
struct TwitterCom;

#[derive(Debug)]
struct NitterNet;

#[derive(Debug)]
struct IncompleteUrl<H>(String, PhantomData<H>);

impl<H, S: Into<String>> From<S> for IncompleteUrl<H> {
    fn from(value: S) -> Self {
        Self(value.into(), PhantomData)
    }
}

impl<H> IncompleteUrl<H> {
    #[allow(dead_code)]
    fn incomplete_url(&self) -> &str {
        &self.0
    }
}

impl IncompleteUrl<TwitterCom> {
    fn real_url(&self) -> String {
        format!("https://twitter.com{}", self.0)
    }
}

impl IncompleteUrl<NitterNet> {
    fn real_url(&self) -> String {
        format!("https://nitter.net{}", self.0)
    }
}

#[derive(Debug)]
struct TwitterStatus {
    timeline: Vec<Tweet>,
    fullname: String,
}

#[derive(Debug)]
struct Tweet {
    url: IncompleteUrl<TwitterCom>,
    is_retweet: bool,
    is_quote: bool,
    #[allow(dead_code)]
    is_pinned: bool,
    date: NaiveDateTime,
    content: String,
    attachments: Vec<Attachment>,
}

#[derive(Debug)]
enum Attachment {
    Image(Image),
    Video(Video),
}

#[derive(Debug)]
struct Image {
    url: IncompleteUrl<NitterNet>,
}

#[derive(Debug)]
struct Video {
    preview_image_url: IncompleteUrl<NitterNet>,
}

pub struct TwitterComFetcher {
    params: PlatformTwitterCom,
    nitter_host: String,
}

impl Fetcher for TwitterComFetcher {
    fn fetch_status(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<Status>> + Send + '_>> {
        Box::pin(self.fetch_status_impl())
    }
}

impl fmt::Display for TwitterComFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.params)
    }
}

impl TwitterComFetcher {
    pub fn new(params: PlatformTwitterCom) -> Self {
        Self {
            params,
            nitter_host: Config::platform_global()
                .twitter_com
                .as_ref()
                .map(|t| t.nitter_host.as_str())
                .unwrap_or("https://nitter.net")
                .trim_end_matches('/')
                .into(),
        }
    }

    async fn fetch_status_impl(&self) -> anyhow::Result<Status> {
        let status = fetch_twitter_status(&self.nitter_host, &self.params.username).await?;

        let posts = status
            .timeline
            .into_iter()
            .map(|tweet| Post {
                content: tweet.content,
                url: tweet.url.real_url(),
                is_repost: tweet.is_retweet,
                is_quote: tweet.is_quote,
                attachments: tweet
                    .attachments
                    .into_iter()
                    .map(|attachment| match attachment {
                        Attachment::Image(image) => PostAttachment::Image(PostAttachmentImage {
                            media_url: image.url.real_url(),
                        }),
                        // For now, we have no way to get the URL of the video, so we convert the
                        // preview image of the video into an image attachment.
                        //
                        // TODO: Add an overlay on the preview image to indicate that it's a video.
                        Attachment::Video(video) => PostAttachment::Image(PostAttachmentImage {
                            media_url: video.preview_image_url.real_url(),
                        }),
                    })
                    .collect(),
            })
            .collect();

        Ok(Status {
            kind: StatusKind::Posts(Posts(posts)),
            source: StatusSource {
                platform_name: PlatformName::TwitterCom,
                user: Some(StatusSourceUser {
                    display_name: status.fullname,
                    profile_url: format!("https://twitter.com/{}", self.params.username),
                }),
            },
        })
    }
}

async fn fetch_twitter_status(
    nitter_host: &str,
    username: impl AsRef<str>,
) -> anyhow::Result<TwitterStatus> {
    let resp = reqwest::ClientBuilder::new()
        .gzip(true)
        .user_agent(prop::PACKAGE.user_agent)
        .build()
        .map_err(|err| anyhow!("failed to build client: {err}"))?
        .get(format!("{nitter_host}/{}", username.as_ref()))
        .header(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.5"))
        .send()
        .await
        .map_err(|err| anyhow!("failed to send request: {err}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("response status is not success: {resp:?}");
    }

    let text = resp
        .text()
        .await
        .map_err(|err| anyhow!("failed to obtain text from response: {err}"))?;

    parse_nitter_html(text)
}

macro_rules! s {
    ( $elem:ident.select($selector:literal)$(.attr($attr:literal))* ) => {{
        static SELECTOR__: Lazy<Selector> = Lazy::new(|| Selector::parse($selector).unwrap());
        $elem.select(&SELECTOR__)
            .next()
            $(.and_then(|a| a.attr($attr)))*
            .ok_or_else(|| anyhow!("selector '{}' doesn't match any element", $selector))
    }};
    ( @SUB: $elem:expr,  $(,)? ) => { $elem };
    ( $elem:ident.selects($selector:literal) ) => {{
        static SELECTOR__: Lazy<Selector> = Lazy::new(|| Selector::parse($selector).unwrap());
        $elem.select(&SELECTOR__)
    }};
    ( $elem:ident.contains($selector:literal) ) => {{
        static SELECTOR__: Lazy<Selector> = Lazy::new(|| Selector::parse($selector).unwrap());
        $elem.select(&SELECTOR__).next().is_some()
    }};
}

fn parse_nitter_html(html: impl AsRef<str>) -> anyhow::Result<TwitterStatus> {
    let html = Html::parse_document(html.as_ref());

    let mut timeline = vec![];
    let fullname = s!(html.select(".profile-card-fullname").attr("title"))?;

    for timeline_item in s!(html.selects(".timeline-item")) {
        let tweet_link =
            s!(timeline_item.select(".tweet-link").attr("href"))?.trim_end_matches("#m");
        let tweet_body = s!(timeline_item.select(".tweet-body"))?;

        let is_pinned = s!(tweet_body.contains(".pinned"));
        let is_retweet = s!(tweet_body.contains(".retweet-header"));
        let is_quote = s!(tweet_body.contains(".quote"));
        let tweet_date = s!(tweet_body.select(".tweet-date > a").attr("title"))?;
        let tweet_content = s!(tweet_body.select(".tweet-content"))?.text().collect();
        let attachment_images = s!(tweet_body.selects(".attachment.image > .still-image"))
            .filter_map(|image| -> Option<Attachment> {
                image
                    .attr("href")
                    .map(|url| Image { url: url.into() })
                    .map(Attachment::Image)
                    .or_else(|| {
                        error!("[twitter.com] '{tweet_link}' has image without href");
                        None
                    })
            });
        let attachment_videos = s!(tweet_body.selects(".attachment.video-container > img"))
            .filter_map(|video| -> Option<Attachment> {
                Some(Attachment::Video(Video {
                    preview_image_url: video.attr("src").unwrap().into(),
                }))
            });

        let tweet_date =
            NaiveDateTime::parse_from_str(tweet_date.trim(), "%b %-d, %Y · %-I:%M %p UTC")
                .map_err(|err| anyhow!("failed to parse tweet date: {err}"))?;

        let tweet = Tweet {
            url: tweet_link.into(),
            is_retweet,
            is_quote,
            is_pinned,
            date: tweet_date,
            content: tweet_content,
            attachments: attachment_images.chain(attachment_videos).collect(),
        };

        timeline.push(tweet);
    }

    // Pinned tweet is always the first tweet in the timeline, so let's sort the
    // timeline by date.
    timeline.sort_by(|lhs, rhs| rhs.date.cmp(&lhs.date));

    Ok(TwitterStatus {
        timeline,
        fullname: fullname.into(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[tokio::test]
    async fn test_twitter_status() {
        let year_2024 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let status = fetch_twitter_status("https://nitter.privacydev.net", "nasa")
            .await
            .unwrap();

        assert_eq!(status.fullname, "NASA");

        let timeline_iter = || status.timeline.iter();

        assert!(timeline_iter().all(|tweet| tweet.date.date() > year_2024));
        assert!(timeline_iter().all(|tweet| !tweet.content.is_empty()));
        assert!(timeline_iter().any(|tweet| !tweet.attachments.is_empty()));
        assert!(timeline_iter().any(|tweet| tweet.url.incomplete_url().contains("/NASA/")));
    }
}
