# live-stream-watcher

Watch the status of your favorite lives and push notifications to your configured chats.

## Supported platforms

### Live streaming

- [bilibili](https://live.bilibili.com/)

### Notification

- [Telegram](https://telegram.org/)

Yea! PRs for support of more platforms are welcome!

## Self-Host

### 1. Configure

Create a configuration file with the following format:

```toml
interval = '1min' # update interval for each subscription

[notify.Personal] # define a target of notifications with name `Personal`
telegram = [ { username = "my_follows", thread_id = 114, token_env = "PERSONAL_TELEGRAM_BOT_TOKEN" } ] # notifications will be pushed to 1 Telegram chat according to the given parameters

[notify.Suzume] # define a target of notifications with name `Suzume`
telegram = [ { id = 1145141919, token = "1234567890:AbCdEfGhiJkLmNoPq1R2s3T4u5V6w7X8y9z" } ]

[[subscription.Suzume]] # define a subscription with name `Suzume`
platform = "live.bilibili.com" # specify the live streaming platform
uid = 6610851 # parameters specific to different live streaming platforms
notify = "Suzume" # reference to notify defined above, notifications will be pushed when the live status changed

[[subscription.CookieBacon]] # define a subscription with name `CookieBacon`
platform = "live.bilibili.com"
uid = 14172231
notify = "Personal"
```

> [!NOTE]
> This project is in an initial development phase, ths configuration may frequently undergo breaking changes in releases.

### 2. Build and Run

```bash
git clone https://github.com/SpriteOvO/live-stream-watcher.git
cd live-stream-watcher
git checkout <latest-version>

cargo build --release
./target/release/live-stream-watcher --config "path/to/config.toml"
```

## License

This project is licensed under [GNU AGPL-3.0 License](/LICENSE).
