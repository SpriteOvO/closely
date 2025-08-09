# Closely

[![](https://img.shields.io/github/actions/workflow/status/SpriteOvO/closely/CI.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white)](https://github.com/SpriteOvO/closely/actions/workflows/CI.yml)

Subscribe to updates from people you follow, from any platform to any platform.

## Supported platforms

### Source of update

- Social media
  - [Twitter (twitter.com)](https://twitter.com/)
  - [bilibili 动态 (t.bilibili.com)](https://t.bilibili.com/)
  - [bilibili 视频 (space.bilibili.com)](https://space.bilibili.com/)

- Live streaming
  - [bilibili 直播 (live.bilibili.com)](https://live.bilibili.com/)
  - [bilibili 录播 (BililiveRecorder)](https://rec.danmuji.org/)

### Notification target

- [QQ](https://im.qq.com/)
- [Telegram](https://telegram.org/)

Yea! PRs for support of more platforms are welcome!

## Self-Host

### 1. Configure

Create a configuration file with the following example format:

```toml
interval = '1min' # update interval for each subscription

# setup platform global configuration
[platform.Twitter.account.MyTwitterAccount]
cookies = "..."

[notify]
# define a target of notifications with name `Personal`
# notifications will be pushed to Telegram chat `@my_follows` under thread ID `114`
Personal = { platform = "Telegram", username = "my_follows", thread_id = 114, token_env = "PERSONAL_TELEGRAM_BOT_TOKEN" }
# define a target of notifications with name `SuzumeChannel`
SuzumeChannel = { platform = "Telegram", id = 1145141919, token = "1234567890:AbCdEfGhiJkLmNoPq1R2s3T4u5V6w7X8y9z" }

[[subscription.Suzume]] # define a subscription with name `Suzume`
# specify the platform and parameters
platform = { name = "bilibili.live", user_id = 6610851 }
# reference to notify defined above, notifications will be pushed when the status changed
notify = ["SuzumeChannel"]

[[subscription.Suzume]]
platform = { name = "Twitter", username = "suzumiyasuzume", as = "MyTwitterAccount" }
notify = ["SuzumeChannel", "Personal"]

[[subscription.CookieBacon]] # define a subscription with name `CookieBacon`
platform = { name = "bilibili.live", user_id = 14172231 }
interval = '30s' # optional, override the global interval value for this individual subscription
# use `Personal` as the notification target, but with the parameter `thread_id = 514` overridden
notify = [ { to = "Personal", thread_id = 514 } ]
```

> [!NOTE]
> This project is in an initial development phase, the configuration format may frequently undergo breaking changes in releases.
>
> The above example is incomplete, and there is currently no documentation. Please check the code for unit tests related to configuration for more detailed usage, and [open an issue](https://github.com/SpriteOvO/closely/issues/new) if you need help.

### 2. Build and Run

```bash
git clone https://github.com/SpriteOvO/closely.git
cd closely
git checkout <latest-version>

cargo build --release
./target/release/closely --config "path/to/config.toml"
```

## License

This project is licensed under [GNU AGPL-3.0 License](/LICENSE).
