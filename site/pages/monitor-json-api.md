title: Monitor a JSON API for changes
description: Watch one field of a JSON API, or the whole response, and get a message or a signed webhook when it changes or meets a condition. With an HTTP API and MCP server.
name: JSON APIs
card: One field of a response, or all of it, to a signed webhook.
tile: tile-json
---
# Monitor a JSON API for changes

Zetlyn reads a JSON response as one line per value, each with its path beside it:

```
assets[0].name: zetlyn-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
draft: false
tag_name: v1.0.0
```

Each value is a place you can watch, like a price on a web page.

## Watch one field

Through the API:

```
curl -s https://zetlyn.com/api/v1/preview \
  -H "Authorization: Bearer $ZETLYN_KEY" -H "Content-Type: application/json" \
  -d '{"url": "https://api.github.com/repos/rust-lang/rust/releases/latest", "find": "tag_name"}'
```

The answer has the field's `selector`. Pass it to `POST /watches`, with a condition if you want
one: `{"below": 400}`, `{"above": 1000}`, `{"contains": "stable"}`, `{"not_contains": "1.99"}`.

## Watch the whole response

Without a selector Zetlyn compares the whole document line by line. Fields are in the order of
their names, so an API that only reorders its fields is no change.

## Get it as a webhook

Under Notifications, add a webhook: each change is posted to your address as JSON, signed with
`Zetlyn-Signature: t=…,v1=…` (HMAC-SHA256 of `t.body`). The format is in [the docs](/docs) and in
[openapi.json](/openapi.json).

Only public addresses are read; an API that needs a key of yours is not.

::cards /release-monitor /rss-feed-monitor /website-change-monitor
