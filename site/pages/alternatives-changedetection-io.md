title: Zetlyn and changedetection.io compared
description: Zetlyn and changedetection.io side by side: hosted price, self-hosting, check intervals, notifications, JSON and feeds, AI assistants. Facts from both sites, 21 October 2026.
name: changedetection.io
card: Hosted or self-hosted, open source on both sides.
---
# Zetlyn and changedetection.io compared

Both are open source and can be self-hosted, and both have a hosted service. changedetection.io is
the larger project, with more ways to filter and more notification services. Zetlyn is smaller, and
built around one value before and after, conditions, and use from AI assistants.

changedetection.io's figures are from its site and README as of 21 October 2026.

| | Zetlyn | changedetection.io |
|---|---|---|
| Licence | MIT | Apache-2.0 |
| Run it yourself | one Rust binary and one SQLite file, or Docker | Docker, Python |
| Hosted | 9.99 € a month, VAT included, 10 watches | $8.99 a month, up to 5,000 URLs |
| Free | 1 watch for 3 days | self-hosted |
| Most often | every minute | from about 5 minutes (hosted) |
| Pages made by scripts | read in Chrome | read in Chrome |
| Notifications | email, Telegram, Slack, Discord, signed webhook | over 85 kinds |
| Filters | a place by clicking it; conditions below, above, contains, no longer contains | CSS, XPath, JSONPath, jq, text triggers |
| JSON and feeds | JSON fields and RSS/Atom entries as places | JSON API monitoring with JSONPath or jq |
| AI assistants | MCP server with OAuth | LLM summaries of changes |

## When Zetlyn fits

- You want an assistant such as Claude or ChatGPT to make and change watches for you.
- You want the value before and after, and a message only when a condition comes true.
- You want one binary and one file on your own server.

## When changedetection.io fits

- You watch hundreds or thousands of pages on the hosted plan.
- You need XPath, jq or one of its many notification services.

[Visualping](/alternatives/visualping) · [Distill](/alternatives/distill) ·
[All use cases](/use-cases)
