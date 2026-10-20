# Listing Zetlyn in MCP directories

What to paste where. The server is `https://zetlyn.com/mcp` (Streamable HTTP, OAuth 2.1 with
dynamic client registration and PKCE). Icons: `icon-512.png`, `icon-64.png` (OpenAI: 64 × 64,
under 5 KB).

## Common texts

- **Name:** Zetlyn
- **One-liner (≤ 200):** Watch any public web page, or one place on it such as a price, stock or a
  version, and get told by email, Telegram, Slack or webhook when it changes.
- **Short (≤ 100):** Watch a place on a web page (a price, stock, a version) or a whole page; get
  told when it changes.
- **Description (≤ 2000):**

  Zetlyn watches public web pages for you. Ask Claude to keep an eye on a price, an availability,
  a release version, a date or a whole page, and Zetlyn reads it on its own servers every hour,
  or as often as every minute with a subscription, and tells you when it changes: by email,
  Telegram, Slack, Discord or a signed webhook. It reports what was there before and what is
  there now, not a diff to decipher.

  You can also say when it is worth telling: only when a price falls below a mark, when "In stock"
  appears, or when "Sold out" goes. Clocks, counters and rotating adverts are recognised and not
  reported; a box that flips between two texts on its own is learned and left out.

  Claude previews a page and picks the place that matters, tries places before watching them,
  changes a watch in place, and lists what changed lately across all your watches. Pages behind
  a login, a paywall or a captcha are not read, and sites that refuse programs are not worked
  around.

  The first watch is free for 3 days. A subscription is 9.99 € a month for 10 watches
  (0.50 € for each one more), VAT included; Claude gives you a link to subscribe, and you pay at
  Stripe yourself.

- **Categories:** Productivity, Monitoring / Alerts, Shopping, Developer tools
- **Website:** https://zetlyn.com
- **Documentation:** https://zetlyn.com/docs
- **Privacy policy:** https://zetlyn.com/privacy
- **Terms:** https://zetlyn.com/terms
- **Support:** hello@zetlyn.com
- **Company:** Artur Susdorf, Zetlyn (see https://zetlyn.com/legal)
- **Reads or writes:** both: reads pages and the account's watches; makes, changes and ends watches.
- **Data handling:** our own API; no personal health data, no sponsored content.

## Example prompts

1. "Watch the price of the PS5 Pro on https://www.amazon.de/dp/B0FRYBZR91 and tell me when it
   drops below 700 €." (preview_page, read_place, watch_page with notify_when)
2. "Let me know when a new version of Rust is released on https://www.rust-lang.org." (preview_page,
   watch_page)
3. "What changed on my watched pages this week?" (list_changes)
4. "Check that page every 15 minutes instead, and only tell me when it says In stock."
   (update_watch)
5. "Which pages am I watching, and what do they say now?" (list_watches, get_watch)
6. "Stop watching the Rust page." (end_watch)

Negative cases (should not make a watch): a page behind a login; a site that answers with a
captcha (Zetlyn says it cannot be read and suggests another site); a request to pay for the
person (Claude gives the subscribe link instead).

## Tools and their hints

| Tool | Title | Read only | Destructive |
|---|---|---|---|
| preview_page | Preview a page | yes | no |
| read_place | Read places on a page | yes | no |
| watch_page | Watch a page | no | no |
| update_watch | Change a watch | no | no |
| list_changes | List recent changes | yes | no |
| subscribe_link | Get a link to subscribe | yes | no |
| list_watches | List watches | yes | no |
| get_watch | Get a watch | yes | no |
| end_watch | End a watch | no | yes |
| get_account | Get the account | yes | no |

## Reviewer test account

Sign-in is by an email link, Google or Apple, none of which a reviewer can use, so the account
`review@zetlyn.com` (free, three watches: a version on rust-lang.org, a text condition on
python.org, a price with `below` on books.toscrape.com) is opened by a reviewer link instead:
`https://zetlyn.com/review/<token>`, which signs in to that account, can be opened again, and
works for 60 days. Make a new one with `zetlyn review review@zetlyn.com --days 60` on the server
or in the admin (the account, "Make a reviewer link"); end it there ("End the reviewer links").
The link is a credential: it goes only into the portal's reviewer field, never into the repo.

Text for the reviewer field:

> 1. Open this link in your browser; it signs you in to Zetlyn's review account (no password,
>    no mail): <reviewer link>
> 2. In Claude, add the custom connector `https://zetlyn.com/mcp` and connect it. Zetlyn's page
>    asks you to allow Claude; press Allow.
> 3. All tools can be called. The account is free with room for 10 watches and has three
>    already. Try: "What am I watching on Zetlyn?", "Watch https://www.rust-lang.org/learn and
>    tell me when the book link changes", "End the watch on books.toscrape.com".

## Where

- **MCP registry** (registry.modelcontextprotocol.io): `server.json` in the repository root,
  name `com.zetlyn/zetlyn`; proof of the domain at `https://zetlyn.com/.well-known/mcp-registry-auth`
  (private key: `secrets/mcp-registry-key.pem`). Publish:

      mcp-publisher login http --domain zetlyn.com --private-key "$(openssl pkey -in secrets/mcp-registry-key.pem -noout -text | grep -A3 'priv:' | tail -n +2 | tr -d ' :\n')"
      mcp-publisher publish

  Bump `version` in `server.json` for each new publish. PulseMCP and others read the registry.
- **Claude Connectors Directory:** https://claude.ai/directory/manage → Submit new → MCP connector
  (any paid Claude plan).
- **ChatGPT apps:** OpenAI platform, apps (verified organization needed). Domain proof: set
  `OPENAI_APPS_CHALLENGE=<token>` in `/etc/zetlyn/env`; it is served at
  `/.well-known/openai-apps-challenge`.
- **Smithery** (smithery.ai), **Glama** (glama.ai/mcp/servers), **mcp.so**, **PulseMCP**
  (pulsemcp.com): submit the URL and the texts above.
