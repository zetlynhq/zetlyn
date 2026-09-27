# Zetlyn

Pick a topic. Zetlyn keeps it current.

Everything published about one subject is scattered across the people who publish it. Zetlyn
collects those sources, joins them on the number they already share, and serves the result as one
thing you can search, browse and be notified about.

Two words carry the whole idea.

A **dataset** is one source. A file describes it; after that it fetches itself on its own clock,
indexes itself, notices what changed since last time, and says what it holds and how to ask.

A **scope** is a topic: several datasets, joined on a shared identifier, with a sentence for each
saying what it contributes that the others do not.

One binary. SQLite underneath. No database to run, no account to create, nothing sent anywhere you
did not point it at.

## Install

```
cargo install --git https://github.com/zetlynhq/zetlyn-core
```

Or from a checkout:

```
cargo build --release      # target/release/zetlyn
```

Rust 1.80 or later. `git` on the machine, for a dataset that is a checkout. Nothing else: SQLite is
compiled in.

## Two commands to something browsable

```
zetlyn dataset new --from prices.csv --at datasets/prices --name mine/prices --kind product
zetlyn dataset run datasets/prices
zetlyn serve datasets/prices
```

`new` reads the file, guesses the identifier, the title, the text and the field types, writes
`dataset.toml`, and prints the first three records it would produce. Edit that file; it is the
whole configuration. `serve` opens the dataset itself, with no scope anywhere: an overview, the
views it declares, browse with facets and columns, search by text and by field, a record page.

## A topic

```toml
# scopes/vulns/scope.toml
name = "mine/vulns"

[[members]]
dataset  = "cve/kev"
priority = "primary"
why      = "The only source that says a vulnerability is being exploited right now."

[[members]]
dataset  = "mine/inventory"
why      = "Which of them we actually run."

[[join]]
key = "cve"
```

```
zetlyn scope search scopes/vulns "exploited=yes and severity>=high"
zetlyn serve scopes/vulns
```

A scope holds no index. It rewrites the query for each member, asks them in parallel, merges the
ranked lists, gathers the hits into one entry per subject, and applies the whole filter again over
the assembled entry — because a question like that one is answered by no member alone.

## Taking somebody else's

A dataset travels as bytes: the records, not the instructions for producing them. You need none of
the publisher's credentials and are not subject to the source's rate limits.

```
zetlyn dataset subscribe cve/kev --from https://hub.zetlyn.com
zetlyn scope subscribe zetlyn/cve --from https://hub.zetlyn.com   # and its members
zetlyn dataset update datasets/kev
```

A hub is a directory layout over HTTPS and nothing more. A folder, a mounted drive, an S3 bucket
or a web server is one:

```
zetlyn dataset publish datasets/prices --to /Volumes/share/hub
zetlyn dataset publish datasets/prices --to s3://my-bucket/hub
zetlyn dataset publish datasets/prices --to https://hub.zetlyn.com   # ZETLYN_HUB_TOKEN
```

Only the last needs anybody's permission, because it is the only one where a name is contended
for. `zetlyn hub` runs one of those, and `SPEC.md` says how the layout works.

An update after the first takes only what changed. On `cve/kev`, five records altered out of
1,726: 3,022 bytes against 2,102,623 for the whole, and the same store either way.

Sign what you publish, and let subscribers pin it:

```
zetlyn hub key --at datasets/prices                    # prints the public half
zetlyn dataset subscribe mine/prices --from … --key ed25519:…
```

A hash per payload catches a fetch that went wrong. It does not catch a hub that served something
else on purpose, because whoever serves the payload serves the manifest beside it. The signature
is the part a hub cannot write for you.

## Checking what it claims

```
zetlyn dataset check datasets/prices
zetlyn scope check scopes/vulns
```

An example that returns nothing, a column naming a field no record carries, a join key only one
member has, a promise that no longer holds. None of it is wrong until somebody reads it, which is
why a run never catches it.

## What it does not do

No summaries, no answers, no rewriting a source's prose. Every value keeps the source that said it
and the word that source used; where two sources disagree after the scope's map, both stay and the
field is marked.

## Licence

Apache-2.0. The name is not in the grant: a fork takes the code and takes another name.
