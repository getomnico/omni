# Web Connector

Crawls a website into Omni using [spider](https://github.com/spider-rs/spider).

Enabled with the `web` Docker Compose profile. Configuration is managed through the Omni admin UI at `/admin/settings/integrations`.

## Features

- Depth-limited crawl with subdomain and robots.txt controls
- Blacklist patterns to skip sections of a site
- Content-hash change detection and automatic deletion detection
- Optional [Spider Cloud](https://spider.cloud) crawling for sites behind bot protection

## Source Configuration

| Field | Default | Description |
|---|---|---|
| `root_url` | *required* | URL to start crawling from |
| `max_depth` | `10` | Max link depth |
| `max_pages` | `10000` | Max pages to crawl |
| `respect_robots_txt` | `true` | Honor robots.txt |
| `user_agent` | — | Custom user agent |
| `blacklist_patterns` | `[]` | URL patterns to skip |
| `include_subdomains` | `false` | Follow links to subdomains |

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `WEB_CONNECTOR_PORT` | `4004` | Port exposed by the connector |
| `WEB_SYNC_INTERVAL_SECONDS` | `86400` | Recrawl interval |
| `RUST_LOG` | — | Log level (e.g. `debug`, `info`) |

## Spider Cloud (optional)

Sites behind Cloudflare, a WAF, or IP/geo blocking answer a direct fetch with
403/429/503. Those pages are dropped and never make it into the index.
[Spider Cloud](https://spider.cloud) routes the crawl through rotating proxies
and anti-bot bypass instead, which recovers them.

To enable it, set an API key on the web connector:

```bash
SPIDER_CLOUD_API_KEY=sk-...
```

That is the whole setup — every web source then crawls through Spider Cloud.
**Leave `SPIDER_CLOUD_API_KEY` unset and nothing changes**; the connector uses
the same direct-fetch path it does today.

| Variable | Default | Description |
|---|---|---|
| `SPIDER_CLOUD_API_KEY` | — | API key. Unset disables the integration entirely. |
| `SPIDER_CLOUD_MODE` | `smart` | `smart` \| `proxy` \| `unblocker` \| `api` \| `fallback` |
| `SPIDER_CLOUD_API_URL` | `https://api.spider.cloud` | Override the API base URL |

`smart` proxies every request and automatically escalates to the unblocker API
when it detects bot protection (403/429/503, Cloudflare challenges, CAPTCHA
pages), which gives the highest success rate. `proxy` is proxy transport only.
`fallback` fetches directly first and only reaches for the cloud after a
failure, which is the most cost-conscious option. An unrecognized value falls
back to `smart`.
