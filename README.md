# Closely

[![](https://img.shields.io/github/actions/workflow/status/SpriteOvO/closely/CI.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white)](https://github.com/SpriteOvO/closely/actions/workflows/CI.yml)

Subscribe to updates from people you follow, from any platform to any platform.

## Supported platforms

### Source of update

- Social media
  - [Twitter](https://twitter.com/)

- Live streaming
  - [bilibili live](https://live.bilibili.com/)

### Notification

- [Telegram](https://telegram.org/)

Yea! PRs for support of more platforms are welcome!

## Self-Host

### 1. Configure

Create a configuration file with the following example format:

```toml
interval = '1min' # update interval for each subscription

[notify.Personal] # define a target of notifications with name `Personal`
# notifications will be pushed to 1 Telegram chat according to the given parameters
telegram = [ { username = "my_follows", thread_id = 114, token_env = "PERSONAL_TELEGRAM_BOT_TOKEN" } ]

[notify.Suzume] # define a target of notifications with name `Suzume`
telegram = [ { id = 1145141919, token = "1234567890:AbCdEfGhiJkLmNoPq1R2s3T4u5V6w7X8y9z" } ]

[[subscription.Suzume]] # define a subscription with name `Suzume`
# specify the platform and parameters
platform = { url = "live.bilibili.com", uid = 6610851 }
# reference to notify defined above, notifications will be pushed when the status changed
notify = "Suzume"

[[subscription.Suzume]]
platform = { url = "twitter.com", username = "suzumiyasuzume" }
notify = "Suzume"

[[subscription.CookieBacon]] # define a subscription with name `CookieBacon`
platform = { url = "live.bilibili.com", uid = 14172231 }
notify = "Personal"
```

> [!NOTE]
> This project is in an initial development phase, ths configuration may frequently undergo breaking changes in releases.

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
