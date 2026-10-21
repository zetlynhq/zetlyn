title: RSS and Atom feed monitor
description: Watch an RSS or Atom feed and get one message per new entry, by email, Telegram, Slack, Discord or webhook. Entries that drop off the feed are not reported.
demo: feed
kicker: RSS · Atom · one message per entry
name: RSS and Atom feeds
card: One message for each new entry, not for the whole feed.
tile: tile-feed
---
# RSS and Atom feed monitor

Paste a feed's address and Zetlyn tells you about every new entry: its title and its link. Entries
that drop off the end of the feed, or are edited, are taken in without a message.

## How it works

1. Paste the feed's address below, for example `https://blog.rust-lang.org/feed.xml`.
2. Watch it as a whole page.
3. Choose where the message goes: email, Telegram, Slack, Discord or a signed webhook.

RSS 2.0, RSS 1.0 and Atom are read. Zetlyn checks every hour, or every 15, 5 or 1 minutes with a
subscription.

## Feeds to start with

- Project releases on GitHub: `https://github.com/<owner>/<repo>/releases.atom`
- A blog's feed, usually linked as `feed.xml`, `rss.xml` or `/feed/`
- Security advisories and status feeds

::cards /release-monitor /monitor-json-api /website-change-monitor
