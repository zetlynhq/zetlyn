title: Use Zetlyn in ChatGPT
description: Add Zetlyn to ChatGPT as an MCP connector with its address alone, sign in once, and ask ChatGPT to watch web pages, prices and releases for you.
demo: chat-chatgpt
kicker: Guide · ChatGPT
name: ChatGPT
card: As a connector, with its address alone.
---
# Use Zetlyn in ChatGPT

Zetlyn is an MCP server at `https://zetlyn.com/mcp`, with OAuth: a client needs nothing but the
address, and you sign in to Zetlyn once.

## Connect

Where your ChatGPT plan lets you add your own connectors (MCP servers), add one with the URL
`https://zetlyn.com/mcp`. ChatGPT opens Zetlyn's sign-in, by email link, Google or Apple; allow
ChatGPT, and the tools are there. The exact place of the setting in ChatGPT changes from time to
time; look for connectors or apps in its settings.

## What to ask

- *"Tell me when this product is back in stock."*
- *"Watch the GitHub releases of my framework and tell me about new versions."*
- *"Which pages am I watching, and what do they say now?"*

## Without signing in through ChatGPT

A program, or a client that cannot sign in, uses an API key instead. Make one under Account, API
keys, and send it as `Authorization: Bearer zk_…`. The HTTP API is described in [the docs](/docs)
and [openapi.json](/openapi.json).

[Claude](/docs/claude) · [Cursor](/docs/cursor) · [All use cases](/use-cases)
