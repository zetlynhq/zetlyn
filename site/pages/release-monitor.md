title: Get told when a new software version is released
description: Watch a release page, a GitHub releases feed or a JSON API and get one message when a new version appears. Works with HTML pages, RSS and Atom feeds and JSON. Three days free.
---
# Get told when a new software version is released

A new version of something you depend on shows up in one of three places: a download page, a
release feed, or an API. Zetlyn watches all three.

## A download page

Paste the page's address below and click the version number. Zetlyn tells you when it changes,
with the old and the new version.

## A release feed

Most projects publish their releases as RSS or Atom, GitHub included:

```
https://github.com/rust-lang/rust/releases.atom
```

Watch the feed's address as a whole page. Zetlyn reads a feed as its list of entries and tells you
only about new ones, with title and link. Entries that drop off the end are not reported.

## A JSON API

Zetlyn reads JSON too, one line per field, each field with its path beside it. For GitHub's latest
release:

```
https://api.github.com/repos/rust-lang/rust/releases/latest
```

Watch the field `tag_name`. Through the API or an assistant, `preview_page` with `find:
"tag_name"` gives the place to watch. Add a condition such as **When it no longer says** `1.99`
to be told only when the version moves on.

## Into your own tools

A signed webhook sends each new release as JSON to your own address, for a deploy pipeline, n8n,
Zapier or Make. See [the docs](/docs) for the format.

[Monitor a JSON API](/monitor-json-api) · [RSS feed monitor](/rss-feed-monitor) ·
[All use cases](/use-cases)
