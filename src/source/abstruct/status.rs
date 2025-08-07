use std::{fmt, vec};

use super::{LiveStatus, Notification, NotificationKind, Posts, PostsRef};
use crate::{platform::PlatformMetadata, source::diff};

#[derive(Clone, Debug, PartialEq)]
pub struct StatusSource {
    pub platform: PlatformMetadata,
    pub user: Option<StatusSourceUser>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusSourceUser {
    pub display_name: String,
    pub profile_url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status(Option<StatusInner>);

#[derive(Clone, Debug, PartialEq)]
struct StatusInner {
    kind: StatusKind,
    source: StatusSource,
}

impl Status {
    pub fn empty() -> Self {
        Self(None)
    }

    pub fn new(kind: StatusKind, source: StatusSource) -> Self {
        Self(Some(StatusInner { kind, source }))
    }

    pub fn sort(&mut self) {
        if let Some(StatusInner {
            kind: StatusKind::Posts(posts),
            ..
        }) = &mut self.0
        {
            // Latest first
            posts.0.sort_by(|l, r| r.time.cmp(&l.time));
        }
    }

    pub fn generate_notifications<'a>(&'a self, last_status: &'a Status) -> Vec<Notification<'a>> {
        self.0
            .as_ref()
            .map(
                |status| match (&status.kind, last_status.0.as_ref().map(|s| &s.kind)) {
                    (StatusKind::Live(live_status), Some(StatusKind::Live(last_live_status))) => {
                        let mut notifications = vec![];
                        if live_status.title != last_live_status.title {
                            notifications.push(Notification {
                                kind: NotificationKind::LiveTitle(
                                    live_status,
                                    &last_live_status.title,
                                ),
                                source: &status.source,
                            })
                        }
                        if live_status.kind != last_live_status.kind {
                            notifications.push(Notification {
                                kind: NotificationKind::LiveOnline(live_status),
                                source: &status.source,
                            })
                        }
                        notifications
                    }
                    (StatusKind::Posts(posts), Some(StatusKind::Posts(last_posts))) => {
                        let new_posts = diff::diff_by(&last_posts.0, &posts.0, |l, r| {
                            l.platform_unique_id() == r.platform_unique_id()
                        })
                        .collect::<Vec<_>>();
                        if !new_posts.is_empty() {
                            vec![Notification {
                                kind: NotificationKind::Posts(PostsRef(new_posts)),
                                source: &status.source,
                            }]
                        } else {
                            vec![]
                        }
                    }
                    (_, None) => vec![],
                    (_, _) => panic!("states mismatch"),
                },
            )
            .unwrap_or_default()
    }

    // Sometimes the data source API glitches and returns empty items without
    // producing any errors. If we simply replace the stored value of `Status`, when
    // the API comes back to normal, we will incorrectly generate notifications with
    // all the items as a new update. To solve this issue, call this function, which
    // will always incrementally store the new items and never delete the old items.
    pub fn update_incrementally(&mut self, new: Status) {
        match (&mut self.0, new.0) {
            (None, None) => {}
            (Some(_), None) => {}
            (inner @ None, Some(new)) => *inner = Some(new),
            (Some(stored), Some(new)) => {
                match (&mut stored.kind, new.kind) {
                    (StatusKind::Live(stored), StatusKind::Live(new)) => *stored = new,
                    (StatusKind::Posts(stored), StatusKind::Posts(new)) => {
                        let mut new = diff::diff_by(&stored.0, new.0, |l, r| {
                            l.platform_unique_id() == r.platform_unique_id()
                        })
                        .collect::<Vec<_>>();
                        // We don't care about the order at the moment.
                        stored.0.append(&mut new);
                    }
                    _ => unreachable!("the stored status and the new status kinds are mismatch"),
                }
                stored.source.platform = new.source.platform;
                if let Some(user) = new.source.user {
                    stored.source.user = Some(user);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatusKind {
    Live(LiveStatus),
    Posts(Posts),
}

impl fmt::Display for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live(live_status) => write!(f, "{live_status}"),
            Self::Posts(posts) => write!(f, "{posts}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;
    use crate::source::*;

    #[test]
    fn status_incremental_update_live() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title1".into(),
                streamer_name: "streamer1".into(),
                cover_image_url: "cover1".into(),
                live_url: "live1".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title2".into(),
                streamer_name: "streamer2".into(),
                cover_image_url: "cover2".into(),
                live_url: "live2".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);
    }

    #[test]
    fn status_incremental_update_posts() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content1"),
                urls: PostUrls::new(PostUrl::Identity("id1".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: None,
            },
        ));
        let last = Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        assert_eq!(status, last);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: None,
            },
        ));
        assert_eq!(status, last);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content3"),
                urls: PostUrls::new(PostUrl::Identity("id3".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user2".into(),
                    profile_url: "profile1".into(),
                }),
            },
        ));
        let last = Status::new(
            StatusKind::Posts(Posts(vec![
                Post {
                    user: None,
                    content: PostContent::plain("content1"),
                    urls: PostUrls::new(PostUrl::Identity("id1".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content2"),
                    urls: PostUrls::new(PostUrl::Identity("id2".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
                Post {
                    user: None,
                    content: PostContent::plain("content3"),
                    urls: PostUrls::new(PostUrl::Identity("id3".into())),
                    time: DateTime::UNIX_EPOCH.into(),
                    is_pinned: false,
                    repost_from: None,
                    attachments: vec![],
                },
            ])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user2".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        assert_eq!(status, last);

        status.update_incrementally(Status::empty());
        assert_eq!(status, last);
    }

    #[test]
    #[should_panic]
    fn status_incremental_update_mismatch() {
        let mut status = Status::empty();
        assert!(status.0.is_none());

        let new = Status::new(
            StatusKind::Live(LiveStatus {
                kind: LiveStatusKind::Online { start_time: None },
                title: "title1".into(),
                streamer_name: "streamer1".into(),
                cover_image_url: "cover1".into(),
                live_url: "live1".into(),
            }),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        );
        status.update_incrementally(new.clone());
        assert_eq!(status, new);

        status.update_incrementally(Status::new(
            StatusKind::Posts(Posts(vec![Post {
                user: None,
                content: PostContent::plain("content1"),
                urls: PostUrls::new(PostUrl::Identity("id1".into())),
                time: DateTime::UNIX_EPOCH.into(),
                is_pinned: false,
                repost_from: None,
                attachments: vec![],
            }])),
            StatusSource {
                platform: PlatformMetadata {
                    display_name: "test",
                },
                user: Some(StatusSourceUser {
                    display_name: "user1".into(),
                    profile_url: "profile1".into(),
                }),
            },
        ));
    }
}
