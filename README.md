# Zetlyn

**Paste a URL. Hear when it changes.** Zetlyn watches a public web page, or one place on it (a
price, a version, a date), and tells you when it changes: the value before and the value now, by
email, Telegram, Slack, Discord or a signed webhook, or only when a condition comes true (a price
below a mark, "In stock" appearing). It is one Rust program with one SQLite file.

- **Hosted:** [zetlyn.com](https://zetlyn.com), the first watch free for 3 days. What people use
  it for: [use cases](https://zetlyn.com/use-cases).
- **For LLMs and agents:** an MCP server at `/mcp` (OAuth 2.1, so Claude, ChatGPT and others
  connect by its address alone) and a JSON API at `/api/v1`, described in
  [`site/llms.txt`](site/llms.txt), [`/docs`](https://zetlyn.com/docs) and
  [`/openapi.json`](https://zetlyn.com/openapi.json). Listed in the MCP registry as
  `com.zetlyn/zetlyn`. Setting it up: [Claude](https://zetlyn.com/docs/claude),
  [ChatGPT](https://zetlyn.com/docs/chatgpt), [Cursor](https://zetlyn.com/docs/cursor).
- **Yours to run:** MIT licensed. A single binary, or the Docker image.

How it reads pages:

- A place is marked by clicking it on the page; Zetlyn finds it again by its path *and* the text
  beside it, and asks to mark it again rather than report a change it is not sure of.
- A whole page is compared as text; what moves on its own (clocks, counters, adverts) is learned
  when the watch begins and when a change is not confirmed by a second read. A place that flips
  between two texts on its own is learned the same way.
- Pages their scripts make are read in a headless Chrome, driven over a pipe; nothing that is not
  on the public internet is ever fetched (`src/outbound.rs`).
- Accounts are an email address and a link (or Google, Apple), no password. On zetlyn.com the
  first watch is free for 3 days; a subscription at Stripe keeps up to 10 watches running.

## Quick start

With Docker, for yourself on your machine (one account, nobody signs in):

```sh
docker run -p 127.0.0.1:4848:4848 -v zetlyn:/data ghcr.io/zetlynhq/zetlyn
```

Open http://127.0.0.1:4848, paste an address, mark a place, press "Watch it". Without Docker,
take the binary for your system from the [releases](https://github.com/zetlynhq/zetlyn/releases)
and run `zetlyn serve --local`; with a Chrome or Chromium installed, pages their scripts make
are read too.

## Build from source

Requirements: a current stable Rust (built and tested with 1.93). SQLite and TLS are built in;
nothing else to install.

```sh
ZETLYN_ADMINS=you@example.com cargo run -- serve
```

This is the service as it runs on zetlyn.com. Open http://127.0.0.1:4848, paste an address,
mark a place, press "Watch it": an email address is asked for, the account comes about with it,
and the watch waits until the address is confirmed. Without SMTP settings no mail is sent: each
one is written to `./data/outbox/*.eml`. Open the newest and follow its link (it points at
`http://127.0.0.1:4848/c/…`) to confirm, start the watch and sign in; signed in, further watches
start without a mail. Without Stripe settings the free trial works and subscribing says "not set
up". The data is in `./data/zetlyn.db`.

Useful while trying it:

```sh
ls -t data/outbox | head -1                                     # the newest mail
grep -ho 'http://127.0.0.1:4848/[cs]/[0-9a-f]*' data/outbox/*.eml  # its links
```

### Admin

The accounts `ZETLYN_ADMINS` names (by commas) see "Admin" in the header, `/admin`: figures,
accounts (sign-in link, free trial, make free without Stripe, watches in the plan, block, end
sessions and keys, delete), every watch (check now, interval, end, delete), read quality by site,
the mail log, billing compared with Stripe, system and a notice over every page, and the log of
everything done there. Anybody else gets "Not here".

### Pages their scripts make

Where a Chrome or Chromium is installed (or `ZETLYN_CHROME` names one), Zetlyn starts it headless
when a page first needs it and drives it over a pipe (`src/render.rs`): the frame to mark in and
the API's preview show the page as a browser does once its scripts ran. A watch is read that way
only when the page as served lacks what was marked, or says clearly less; otherwise it is read as
served, which costs a fraction. Chrome loads no images, fonts or media and nothing that is not on
the public internet; in the frame they come through Zetlyn (`/r`) into the reader's browser. It
ends with Zetlyn.

### One account, no sign-in

```sh
cargo run -- serve --local
```

One account nobody signs in to, no mail, no payment: a watch starts at once. Your watches and
"Check all now" are under "Watches" (`/watches`), and `/admin` is open.

### API and MCP

Under "Account" (`/account`) → "For programs and LLMs", make a key (`zk_…`), then:

```sh
KEY=zk_...
curl -s -H "Authorization: Bearer $KEY" http://127.0.0.1:4848/api/v1/account
curl -s -H "Authorization: Bearer $KEY" -d '{"url":"https://www.iana.org/help/example-domains"}' \
     http://127.0.0.1:4848/api/v1/preview
curl -s -H "Authorization: Bearer $KEY" \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' http://127.0.0.1:4848/mcp
```

All calls and tools are in [`site/llms.txt`](site/llms.txt), which zetlyn.com serves at `/llms.txt`.

## Run it for others

Without `--local` an instance has accounts: people sign in by a link in a mail (and with Google
or Apple where they are set up), and each sees only their own watches. What it needs:

1. **An address and TLS.** Run it behind a reverse proxy (Caddy, nginx) that hands every request
   to `127.0.0.1:4848`, and tell it its address: `zetlyn serve --base https://watch.example.com`.
2. **Mail.** `SMTP_HOST` and the rest (below); without them mails are written to
   `<data>/outbox`, which is fine for trying it and nothing else.
3. **Your own pages.** The legal notice, privacy, terms and cancellation pages are the operator's
   to write: put `legal.html`, `privacy.html`, `terms.html` and `withdrawal.html` in
   `<data>/site/` and they are shown at `/legal` and the rest. Until then those pages say that
   they are not written yet. (zetlyn.com's own are in `site/` and shown only on zetlyn.com.)
4. **Payment, if any.** Without Stripe settings every account has the free trial and nothing can be
   bought; `ZETLYN_ADMINS` can make accounts free without Stripe in `/admin`.

The units in `deploy/` (systemd with `DynamicUser`, no access to private networks, an alarm) are
how zetlyn.com runs, and a good start for your own; `deploy/README.md` describes them.

## Commands

```
zetlyn serve [--bind 127.0.0.1:4848] [--every 60] [--base https://zetlyn.com] [--local]
zetlyn check                          check every running watch now
zetlyn add <url> [--select <css>]     watch a page from the command line (the place a selector names)
zetlyn add-list <file>                one watch per line: <url> [<css>]
zetlyn tally                          how the checks came out, by outcome
zetlyn backup <file>                  a consistent copy of the data, while it runs
zetlyn alert [--to alarm@zetlyn.com]  mail when the app does not answer or its checks stopped
zetlyn name <instance>                name this instance (one per server)
zetlyn stripe-setup [--archive-others]  make the product and prices at Stripe, print their ids
zetlyn review <email> [--days 30]     an account for a directory's reviewers and a link that signs in to it
zetlyn telegram-setup                 point the Telegram bot (below) at this instance
```

Every command takes `--data <dir>` (default `./data`).

## Settings (environment)

| Variable | For |
|---|---|
| `SMTP_HOST`, `SMTP_PORT` (465 = TLS, else STARTTLS), `SMTP_USER`, `SMTP_PASSWORD_FILE`, `MAIL_FROM` | sending mail; without `SMTP_HOST` mail goes to `<data>/outbox` |
| `STRIPE_API_KEY` (restricted key), `STRIPE_PRICE_BASE`, `STRIPE_PRICE_EXTRA`, `STRIPE_WEBHOOK_SECRET` | the subscription; without them nothing can be bought |
| `ZETLYN_ADMINS` | the addresses, by commas, whose accounts may use `/admin` |
| `GOOGLE_CLIENT_FILE` (the JSON from Google's console; locally `secrets/google-client.json` is found without it, and git ignores `secrets/`), or `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` | "Continue with Google" on the sign-in page; Google's redirect URI is `<base>/oauth/google/back` |
| `APPLE_CLIENT_ID` (Services ID), `APPLE_TEAM_ID`, `APPLE_KEY_ID`, `APPLE_KEY_FILE` (.p8) | "Continue with Apple"; Apple's return URL is `<base>/oauth/apple/back` (HTTPS only) |
| `ZETLYN_CHROME` | the Chrome or Chromium that renders pages whose content their scripts make; `off` for none; without it the usual places are looked in |
| `ZETLYN_CHROME_SANDBOX=off` | start Chrome without its own sandbox; only where something else is one (the Docker image sets it) |
| `TELEGRAM_BOT_TOKEN` (or `TELEGRAM_BOT_TOKEN_FILE`), `TELEGRAM_BOT_NAME` | changes told in Telegram, by a bot of your own from @BotFather; then run `zetlyn telegram-setup --base <base>` |
| `OPENAI_APPS_CHALLENGE` | the token OpenAI gives to prove the domain, served at `/.well-known/openai-apps-challenge` |

Stripe's webhook is `<base>/billing/stripe` with `checkout.session.completed`,
`customer.subscription.updated` and `customer.subscription.deleted`. `zetlyn stripe-setup` (with
`STRIPE_API_KEY` set) makes the product and the two prices, VAT included, and prints the two ids.

## Tests

```sh
cargo test
```

The tests need no network: pages are given as HTML, mails go to a temporary outbox, Stripe's
events are signed in the test. What is not tested automatically is reading real pages; for that
there is a trial of 50 public pages:

```sh
cargo run -- add-list trial/pages.txt --data /tmp/zetlyn-trial
cargo run -- check --data /tmp/zetlyn-trial      # again later
cargo run -- tally --data /tmp/zetlyn-trial
```

On zetlyn.com the same trial runs under the instance's local account.

## Where things are

| Path | What |
|---|---|
| `src/anchor.rs` | a marked place: described, found again, the places on a page (`outline`) |
| `src/check.rs`, `src/text.rs`, `src/fetch.rs`, `src/render.rs`, `src/page.rs`, `src/outbound.rs` | reading a page, as served or rendered in Chrome (never a private address), comparing, the frame to mark in |
| `src/store.rs` | the SQLite file: accounts, watches, checks, changes, links, sessions, API keys |
| `src/life.rs`, `src/plan.rs`, `src/billing.rs` | a watch's life, the free trial and the subscription, Stripe |
| `src/when.rs`, `src/tell.rs` | conditions for telling a change; Telegram, Slack, Discord and webhooks |
| `src/mail.rs` | SMTP and the mails |
| `src/serve.rs`, `src/serve/admin.rs`, `src/serve/authz.rs`, `src/api.rs`, `src/docs.rs` | the site, the admin pages, OAuth for MCP clients, the API and MCP, `/docs` and `/openapi.json` |
| `src/alert.rs`, `src/cli.rs` | the alarm, the command line |
| `assets/` | the stylesheet, the mark, the favicon (carried in the binary) |
| `site/` | zetlyn.com's legal notice, privacy, terms and withdrawal (shown only there), `llms.txt`, `openapi.json` (carried in the binary) |
| `deploy/` | how zetlyn.com is built and run (`deploy/README.md`), its directory listings |
| `Dockerfile`, `.github/workflows/` | the image, the tests, the releases |
| `trial/pages.txt` | the 50 trial pages |

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Run `cargo test` before sending a change; security reports go
privately, see [SECURITY.md](SECURITY.md). How the code is written here:

- Rust and POSIX shell only. No Python, not even for a one-off script.
- Commit messages say what changed and why, and carry no AI attribution lines.
- When the version changes, commit `Cargo.toml` and `Cargo.lock` together; the build runs `--locked`.
- A server, browser or other process started for a test is stopped again in the same step.

## License

MIT, see [LICENSE](LICENSE). The name Zetlyn and its mark belong to zetlyn.com ([NOTICE](NOTICE)); the licences of the crates it is built from are in [THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES).
