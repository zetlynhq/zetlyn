# Zetlyn

Pick a topic. Zetlyn keeps it current.

Everything published about one subject is scattered across the people who publish it. Zetlyn
collects those sources, joins them on the number they already share, and serves the result as one
thing you can search, browse and be notified about.

Two words carry the whole idea.

A **source** is one place that publishes: a file, a feed, an API, a list on a web page. A file describes it; after that it
fetches itself on its own clock, indexes itself, notices what changed since last time, and says what
it holds and how to ask.

A **tracker** is a topic: several sources, joined on a shared identifier, with a sentence for each
saying what it contributes that the others do not.

One binary. SQLite underneath. No database to run, no account to create, nothing sent anywhere you
did not point it at.

## Install

```
curl -fsSL https://zetlyn.com/install.sh | sh
```

macOS (Apple silicon or Intel) or Linux on x86-64: the binary of the latest release, checked
against its checksums, in `~/.local/bin` (or `$ZETLYN_BIN`). Each archive carries LICENSE, NOTICE
and THIRD_PARTY_LICENSES. From source:

```
cargo install --git https://github.com/zetlynhq/zetlyn
```

Or from a checkout:

```
cargo build --release      # target/release/zetlyn
```

Rust 1.82 or later. `git` on the machine, for a source that is a checkout. Nothing else: SQLite is
compiled in.

With an assistant, give it <https://zetlyn.com/llms.txt>: what Zetlyn is, its files, its commands,
and three tasks as they ran, tried by agents that had nothing else.

## No command at all

```
zetlyn
```

opens the workspace in a browser: the current directory if it is one, `~/zetlyn` otherwise. The
page asks what you want to track, then for a first source, an address or a file. It reads the
whole of it, says what names a claim (a CVE, a DOI, an ISBN: fifteen schemes are known by pattern
and check digit, with no model involved), and shows the first claims. A second source is read the same
way, and before anything is connected you see how many things the two share, via which
identifier. **Connect**, and the tracker is there, served as it would be published. Or start from
the example: CISA's exploited vulnerabilities and Exploit-DB, two pastes, and 450 of CISA's 1,728
turn out to have public code.

Nothing here is a second way of doing things: every step is `source new`, `source update` and a
`tracker.yaml`, written where you can read and edit them.

## Two commands to something browsable

```
zetlyn source new --from prices.csv --at sources/prices --name mine/prices --kind product
zetlyn source update sources/prices
zetlyn serve sources/prices
```

`new` reads the file, guesses the identifier, the title, the text and the property types, writes
`source.yaml`, and prints the first three claims it would produce. Edit that file; it is the
whole configuration. `serve` opens the source itself, with no tracker anywhere: an overview, the
views it declares, browse with facets and columns, search by text and by property, a claim page.

A file, a feed, a folder and a list on a web page are read that way, and so are a repository's
releases or advisories (`--from github:<owner>/<repo>/releases`). A JSON API is not: its declaration
is written, `claims: each: field:<list>[]` naming the list in its answer, or proposed by a model
with `zetlyn assist teach <URL>`, which sends nothing before `--send`.

## Every value has a receipt

```
zetlyn claim sources/prices SKU-1042
```

prints the claim, what its source handed over for it (the row, the JSON object, the feed item),
per property the expression that read it and the words it read, and every version it was at with
the time an update first saw it. The pages show the same under each value: who said it, in which
words, since when, and what it said before. A subscriber holds the same receipts as the
publisher, because they travel with the claims. A thing is found by its key as a tracker writes it
(`isbn:9780441172719`) or by its value in any spelling the source uses, and a key written into an
address (`apikey=${KEY}`) is kept in every receipt as the variable, never its value.

## A topic

```yaml
# trackers/vulns/tracker.yaml
name: mine/vulns
sources:
- source: zetlyn/cve-kev
  priority: primary
  why: The only source that says a vulnerability is being exploited right now.
- source: mine/inventory
  why: Which of them we actually run.
identified_by: [cve]
```

```
zetlyn tracker search trackers/vulns "exploited=yes and severity>=high"
zetlyn serve trackers/vulns
```

A tracker holds no index. It rewrites the query for each source, asks them in parallel, merges the
ranked lists, gathers the claims into one thing per identifier, and applies the whole filter again
over the assembled thing — because a question like that one is answered by no source alone.

`zetlyn tracker things trackers/vulns "conflict:severity and has:cve-kev"` asks what a thing is:
which sources speak of it, where they disagree, what appeared or changed lately. It prints each
key and title, a tab between.

## A thing's own page

What a page of one thing says first is the tracker's to declare, since only it knows which of its
properties answer the first question about one of them:

```yaml
thing:
  summary: [severity, cvss, epss, exploited, due_date, fixed_in]
  ladder:
    title: How far exploitation has got
    steps:
    - name: No public code known
    - name: Proof of concept
      when: has:cve-exploitdb
    - name: A Metasploit module
      when: has:cve-metasploit
    - name: Exploited in the wild
      when: exploited=yes
  timeline: [due_date]
```

The summary shows each property with every source that says it, a disagreement marked; the ladder
the steps a thing has reached, each with the source and the day; the timeline the day each source
first spoke of it beside the dates declared. Where two sources score it differently and each wrote
its vector (`cvss_vector`), the page says which metrics they judge differently. Nothing on it is
written by the program: it orders what the sources said, and every value with its receipt is
beneath.

## People as a source

Some things are read by many people and published as data by nobody. A source of
`type: proposals` is those people: each proposes a row they read, with where and when, and the
owner accepts it or does not. Only what is accepted is a claim, and its receipt names who read it.

```yaml
fetch:
  type: proposals
  readers: [signed-in]          # who may propose from the browser; or addresses, domain:…, @<world>
identified_by:
  flight: "const:{flight} {date}"
```

```
zetlyn source proposals sources/flights
zetlyn source accept sources/flights <file>.json
```

## Being told

A watch keeps a thing or a question and tells what changed in its answer: to a feed, by mail,
to a webhook, or to a program of yours, which gets the report as JSON on standard input.

```yaml
# watches/cheap-tokyo.yaml
name: cheap-tokyo
tracker: local/trip
query: price < 600          # or thing: isbn:9780441172719
words: [lh 714]             # only things whose title holds one of these
deliver:
- to: feed                  # served at /watch/cheap-tokyo by zetlyn serve
- to: mail
  address: you@example.org
```

`zetlyn watch check` says what it would tell; `--deliver` tells it; `--from-now` sets aside what
happened before, which a new watch on a tracker that has been running would otherwise tell all of.
`zetlyn run` updates every source on its rhythm, refreshes every tracker and asks every watch.

## Taking somebody else's

A source travels as bytes: the claims, not the instructions for producing them. You need none of
the publisher's credentials and are not subject to the source's rate limits.

```
zetlyn source subscribe zetlyn/cve-kev
zetlyn tracker subscribe zetlyn/cve          # and its sources
zetlyn source update sources/cve-kev      # a subscribed source pulls its next version
```

A reference names a host or it does not, and one that does not means `zetlyn.com`. That is the
whole of the default: `--from` and `--to` are for the other cases. It carries these, and serves two
of the trackers it carries so you can see what one answers before subscribing:
<https://zetlyn.com/zetlyn/trackers/cve/> and <https://zetlyn.com/zetlyn/trackers/local-models/>.
A published tracker's Versions tab says how to take it and which versions there are, and every
source that may be shown has its own page in its world, `https://zetlyn.com/zetlyn/sources/cve-kev/`.
The hub's list leads to where each runs, and beneath it, Elsewhere, the trackers other worlds
describe in their own documents.

A hub is a directory layout over HTTPS and nothing more. A folder, a mounted drive, an S3 bucket
or a web server is one:

```
zetlyn source publish sources/prices --to /Volumes/share/hub
zetlyn source publish sources/prices --to s3://my-bucket/hub
zetlyn source publish sources/prices --to https://hub.example.com
zetlyn source publish sources/prices                          # zetlyn.com
```

Only the last two need anybody's permission, because they are the only ones where a name is
contended for. `zetlyn hub` runs one of those, and `SPEC.md` says how the layout works.

An update after the first takes only what changed. On `zetlyn/cve-kev`, five claims altered out of
1,726: 3,022 bytes against 2,102,623 for the whole, and the same store either way.

## Who you are

One key, everywhere you act: publishing to a hub, operating a workspace, driving a console.

```
zetlyn id new --name "Acme Security" --contact ops@acme.example
zetlyn id
```

It lives in `~/.zetlyn`, or `$ZETLYN_HOME`. There is no account and no service behind it. A key
says who signed something; who that is allowed to be is a hub's owners file or a workspace's
grant, and both of those are somebody's decision about a particular key.

Readers are not this. A person who subscribes to a tracker is an email address in that workspace
and holds no key.

What you publish is signed with it, and subscribers pin it:

```
zetlyn source subscribe mine/prices --from … --key ed25519:…
```

A hash per payload catches a fetch that went wrong. It does not catch a hub that served something
else on purpose, because whoever serves the payload serves the manifest beside it. The signature
is the part a hub cannot write for you.

## Checking what it claims

```
zetlyn source check sources/prices
zetlyn tracker check trackers/vulns
```

An example that returns nothing, a column naming a property no claim carries, an identifier only one
of several sources has, a promise that no longer holds, a key the workspace.yaml does not take.
None of it is wrong until somebody reads it, which is why an update never catches it. Each exits
non-zero when it finds any, and so does `zetlyn source update` when an update was partial or
refused, so a CI stops either.

## A world of your own

Your sources, your trackers and your readers, on a domain of yours, with nobody else's server in
between. Run for you, the same world is at `https://zetlyn.com/<name>/`. On an empty Ubuntu machine, as root, with the domain pointing at it:

```
curl -fsSL https://zetlyn.com/install.sh | ZETLYN_BIN=/usr/local/bin sh
SMTP_PASSWORD=… zetlyn world up prices.example --owner you@example.org \
    --smtp smtp.example.org:587 --smtp-user you@example.org --mail-from "Prices <noreply@prices.example>"
```

That makes a system user, the workspace at `https://prices.example` with you as its owner, Caddy
with a certificate in front of it, and three units: the world, a daily backup of all of it into
`/srv/zetlyn/backups` (`zetlyn world export`, one archive, each database as one moment of itself),
and a daily look for the next release (`zetlyn world upgrade`), which installs it only when it is
newer and its checksum holds. Run it again and it changes only what is missing; `--dry-run` says
what it would do. Without `--smtp` the sign-in links are written to the world's log
(`journalctl -u zetlyn-world`) until a mailer is named in its `workspace.yaml`.

With Docker instead: `DOMAIN=prices.example OWNER=you@example.org docker compose -f
deploy/world/compose.yaml up -d`.

A world describes itself at `https://prices.example/.well-known/zetlyn.json`, signed with its own
key: what it publishes and where (its own hub, at `/hub/`), its public sources and trackers, where
it takes proposals. Anybody who knows only its address can take a source from it:
`zetlyn source subscribe t/prices --from https://prices.example`.

Moving it is three commands:

```
zetlyn world export /srv/zetlyn/world --to world.tar.gz                       # on the old machine
zetlyn world import world.tar.gz --to /srv/zetlyn/world --url https://new.example  # on the new one
zetlyn world move /srv/zetlyn/world --to https://new.example                  # on the old one
```

Everything moves with it: the claims and their receipts, its readers and what they proposed, its
keys. `move` checks that the new address answers as the same world, signed with the same key;
after it, the old address says where it went and redirects every page there, and a subscription
taken from it follows on its next pull.

## Signing in with one world at another

Every world is an OpenID Connect provider for its own people and a relying party for everybody
else's. Somebody who is a reader of one world signs in to another as that, without an account
there: the second world never registered anywhere, because between two zetlyn worlds the client is
the address of a document it serves about itself (`/oauth/client.json`). They are told who they
are by a name the first world gives them, the name they chose, and their address only if they say
so. Discovery is at `<world>/.well-known/openid-configuration`, the flow is the authorization code
with PKCE and nothing else, and ID tokens are signed EdDSA with the world's `oidc.key`.

Who a world takes identities from is `identity:` in its workspace.yaml. Not said, it is "Sign in
with zetlyn.com", which itself takes GitHub, Google and Apple, so a world gets all three without
registering with any:

```yaml
identity:
  - zetlyn: https://zetlyn.com   # the default
  - zetlyn: any                  # whichever world the person names
  - github: { client: ${GITHUB_CLIENT_ID}, secret: ${GITHUB_CLIENT_SECRET} }
  - google: { client: ${GOOGLE_CLIENT_ID}, secret: ${GOOGLE_CLIENT_SECRET}, domain: example.com }
  - apple:  { client: com.example.signin, team: ${APPLE_TEAM_ID}, key_id: ${APPLE_KEY_ID}, key: ${APPLE_PRIVATE_KEY} }
```

A source's `readers` may then name a world or a domain as well as `signed-in` and addresses:
`["@zetlyn.com", "domain:example.com"]`. A proposal made that way says, signed, which world vouched
for whoever made it.

## Who may do what

A world says itself who may do more than read it, in one list, `access:` in its workspace.yaml,
with the same words as a source's `readers`: an address, `domain:<domain>`, `@<world>` for whoever
that world vouches for, or `signed-in`.

```yaml
access:
  owners:    [you@example.org]                 # everything, its settings and its export too
  editors:   ["domain:acme.example"]           # its sources and trackers, and deciding proposals
  proposers: ["@zetlyn.com", "domain:uni-leipzig.de"]   # rows for every source that names nobody itself
```

Everybody else reads what is public. A source that names `readers` of its own is narrower than
the world's proposers; editors and owners propose anywhere. zetlyn.com, or any other world, only
says who somebody is: what they may do is this world's, and a world that takes nobody's word but a
link to an address runs on its own. A hosted world's members on the machine and its own `access:`
count together; `owners:` as written before is an owner still.

## Letting somebody else run it

A workspace can answer for itself, so that whoever keeps it current does not have to be at its
terminal.

```
zetlyn console serve /srv/zetlyn/workspace --port 8100
zetlyn console grant --to ed25519:… --can read,update --until 2027-01-01 --at /srv/zetlyn/workspace
```

The console holds no secret. It holds the public half of your key, and it takes nothing that does
not trace back to it: a grant you signed, and a call signed by the key that grant names. `read`,
`update` and `apply` are the three things a grant can carry, and `apply` is separate because
replacing a declaration and filling a store are different sorts of act.

A grant is not a secret and not a way in on its own. Whoever holds one still has to hold the
private half of the key it names, and a call carries a signature over the method, the path, the
body and the time rather than a token. Editing a grant to give yourself longer stops the signature
describing it.

Whoever holds grants can run a platform over them, which is the same calls with pages on top:

```
zetlyn platform hold --name cve --console https://… --grant cve.yaml --at /srv/platform
zetlyn platform serve /srv/platform --port 8110
```

It holds no claims and no accounts. Everything a page shows was asked for when the page was
asked for, and for a workspace of 98,546 claims what the platform keeps on disk is 831 bytes:
an address and a grant somebody else signed.

Name a command under `draft:` in `platform.yaml` and it can propose a declaration from what the
updates reported they could not make sense of. It proposes; you apply. Zetlyn ships no model and
holds no key for one, the same way it ships no mail client.

## What it does not do

No summaries, no answers, no rewriting a source's prose. Every value keeps the source that said it
and the word that source used; where two sources disagree after the tracker's map, both stay and
the property is marked as a conflict.

## Licence

Apache-2.0. The name is not in the grant: a fork takes the code and takes another name.
