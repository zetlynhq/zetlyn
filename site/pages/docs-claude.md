title: Use Zetlyn in Claude
description: Connect Zetlyn to Claude as a custom connector or in Claude Code, sign in once, and ask Claude to watch web pages for you: prices, stock, versions, whole pages.
demo: chat-claude
kicker: Guide · Claude
name: Claude
card: As a custom connector, or in Claude Code.
---
# Use Zetlyn in Claude

Zetlyn is an MCP server at `https://zetlyn.com/mcp`. Claude connects to it with that address and
signs you in to Zetlyn once, by email link, Google or Apple. After that you ask in plain words and
Claude makes, changes and reads your watches.

## Claude on the web, desktop and mobile

1. Open **Settings → Connectors → Add custom connector**.
2. Enter the URL `https://zetlyn.com/mcp`.
3. Connect, sign in to Zetlyn in the window that opens, and allow Claude.

## Claude Code

```
claude mcp add --transport http --scope user zetlyn https://zetlyn.com/mcp
```

Then run `/mcp` in Claude Code to sign in.

## What to ask

- *"Watch the price on https://books.toscrape.com/catalogue/a-light-in-the-attic_1000/index.html
  and tell me when it drops below 40 £."*
- *"Tell me when a new version of Rust is released."*
- *"What changed on my watched pages this week?"*
- *"Check that page every 15 minutes instead."*
- *"Stop watching the Rust page."*

Claude previews the page, picks the place, and can try places before watching them. It never pays
for you: when a watch needs a subscription, it gives you a link to open yourself.

## The tools

`preview_page`, `read_place`, `watch_page`, `update_watch`, `list_changes`, `list_watches`,
`get_watch`, `end_watch`, `get_account`, `subscribe_link`. All are described in [the
docs](/docs).

[ChatGPT](/docs/chatgpt) · [Cursor](/docs/cursor) · [All use cases](/use-cases)
