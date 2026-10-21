title: Price drop alert for any shop page
description: Get an email, Telegram or Slack message when a price on any web page falls below the mark you set. No browser extension, no account to set up first. Three days free.
name: Price drop alerts
card: When a price falls below the mark you set.
tile: tile-price
demo: price
kicker: A price · a mark · one message
---
# Price drop alert for any shop page

Mark the price on a product page and say below what it is worth telling you. Zetlyn reads the page
every hour, or as often as every minute with a subscription, and tells you once when the price
falls below your mark, with the price before and the price now.

## How it works

1. Paste the product page's address below.
2. Click the price on the page.
3. Choose **When the number is below** and enter your mark, for example `400`.

Zetlyn reads the amount beside the currency, as the page writes it: `1.299,00 €`, `$1,299.99`,
`499,-` and `£51.77` all work. If the place you marked shows two different amounts, such as an old
and a new price, Zetlyn asks you to mark the one you mean, so the alert never compares the wrong
number.

It tells you when the price falls below the mark, not with every cent it falls further. Once the
price has gone back above the mark, the next drop is told again.

## Where the message goes

By email, and if you like to Telegram, Slack, Discord or your own webhook. Every change is also kept
in the watch's history.

::channels

## From Claude or ChatGPT

With Zetlyn connected as an MCP server you can ask in plain words: *"Tell me when the PS5 Pro on
this page drops below 700 €."* The assistant finds the price on the page and makes the watch. See
[Claude](/docs/claude), [ChatGPT](/docs/chatgpt) or [Cursor](/docs/cursor).

## What it does not do

Pages behind a login, a paywall or a captcha are not read, and Zetlyn does not get around a site
that refuses programs. Some large shops do; Zetlyn says so when you try.

::cards /back-in-stock-alert /release-monitor /website-change-monitor
