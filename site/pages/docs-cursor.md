title: Use Zetlyn in Cursor and other MCP clients
description: Add Zetlyn's MCP server to Cursor, VS Code or any MCP client by its URL, sign in once, and let your editor's agent watch pages, feeds and APIs for you.
---
# Use Zetlyn in Cursor and other MCP clients

Add an MCP server with the URL `https://zetlyn.com/mcp`. In Cursor that is an entry in `mcp.json`:

```
{
  "mcpServers": {
    "zetlyn": { "url": "https://zetlyn.com/mcp" }
  }
}
```

The client finds out how to sign in (OAuth with dynamic client registration and PKCE) and opens
Zetlyn's sign-in in your browser. VS Code and other MCP clients work the same way: the address is
all they need.

## With a key instead

Where a client cannot sign in, make an API key under Account, API keys and send it as a header:

```
{
  "mcpServers": {
    "zetlyn": {
      "type": "http",
      "url": "https://zetlyn.com/mcp",
      "headers": { "Authorization": "Bearer zk_…" }
    }
  }
}
```

Keep the key out of anything you commit.

## What to ask

- *"Tell me when a new version of this dependency is released."*
- *"Watch `tag_name` on this API and send it to my webhook when it changes."*
- *"What changed on my watches since yesterday?"*

[Claude](/docs/claude) · [ChatGPT](/docs/chatgpt) · [All use cases](/use-cases)
