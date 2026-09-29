# Zetlyn

Pick a topic. Zetlyn keeps it current.

Everything published about one subject is scattered across the people who publish it. Zetlyn
collects those sources, joins them on the number they already share, and serves the result as one
thing you can search, browse and be notified about.

Two words carry the whole idea.

A **source** is one place that publishes: a file, a feed, an API. A file describes it; after that it
fetches itself on its own clock, indexes itself, notices what changed since last time, and says what
it holds and how to ask.

A **tracker** is a topic: several sources, joined on a shared identifier, with a sentence for each
saying what it contributes that the others do not.

One binary. SQLite underneath. No database to run, no account to create, nothing sent anywhere you
did not point it at.

## Install

```
cargo install --git https://github.com/zetlynhq/zetlyn
```

Or from a checkout:

```
cargo build --release      # target/release/zetlyn
```

Rust 1.80 or later. `git` on the machine, for a source that is a checkout. Nothing else: SQLite is
compiled in.

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

## Every value has a receipt

```
zetlyn claim sources/prices SKU-1042
```

prints the claim, what its source handed over for it (the row, the JSON object, the feed item),
per property the expression that read it and the words it read, and every version it was at with
the time an update first saw it. The pages show the same under each value: who said it, in which
words, since when, and what it said before. A subscriber holds the same receipts as the
publisher, because they travel with the claims.

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

## Taking somebody else's

A source travels as bytes: the claims, not the instructions for producing them. You need none of
the publisher's credentials and are not subject to the source's rate limits.

```
zetlyn source subscribe zetlyn/cve-kev
zetlyn tracker subscribe zetlyn/cve          # and its sources
zetlyn source pull sources/cve-kev
```

A reference names a host or it does not, and one that does not means `hub.zetlyn.com`. That is the
whole of the default: `--from` and `--to` are for the other cases. It carries these, and serves two
of the trackers it carries so you can see what one answers before subscribing:
<https://hub.zetlyn.com/zetlyn/cve> and <https://hub.zetlyn.com/zetlyn/local-models>.

A hub is a directory layout over HTTPS and nothing more. A folder, a mounted drive, an S3 bucket
or a web server is one:

```
zetlyn source publish sources/prices --to /Volumes/share/hub
zetlyn source publish sources/prices --to s3://my-bucket/hub
zetlyn source publish sources/prices --to https://hub.example.com
zetlyn source publish sources/prices                          # hub.zetlyn.com
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
source has, a promise that no longer holds. None of it is wrong until somebody reads it, which is
why an update never catches it.

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
