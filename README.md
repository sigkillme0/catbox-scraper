# catbox-scraper

> **⚠️ this repo is for educational purposes only. i do not recommend running this.**
>
> catbox.moe is an anonymous file host. scraping it **will** surface extreme NSFW, heavy gore, and content that can constitute a felony to possess in most jurisdictions. you have been warned. running this is entirely your own problem.

brute-force scraper for [catbox.moe](https://catbox.moe). generates random 6-char filenames, checks if they exist, downloads hits to `data/<ext>/`.

## usage

```
cargo run --release
```

with proxy it expects `PROXY_HOST`, `PROXY_PORT`, `PROXY_USER`, `PROXY_PASS` in env.

## config

`config.toml`:

```toml
threads = 1500
update_rate = 0.25
file_extensions = ["png", "gif", "jpg", "jpeg", "webm", "mp4"]
```

## how it works

each worker owns a dedicated `reqwest::Client` with a persistent CONNECT tunnel + TLS session. no shared connection pool, no lock contention. ~1200 rps sustained through residential proxies.
