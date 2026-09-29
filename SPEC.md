# The Zetlyn specifications

Version 2.0. What changed from 1.0 is at the end.

Three contracts, and nothing else here is one. The rest of how Zetlyn works is documentation: read
`README.md`, or read the code, which is the same length and more honest.

| | |
|---|---|
| [A claim](#a-claim) | what a source produces |
| [The interface](#the-interface) | the six calls it answers |
| [The hub](#the-hub) | how a source travels, and what the manifest says |

A source somebody else operates has to satisfy the first two to be a source of a tracker. An
artifact somebody else builds has to satisfy the third to be fetched from a hub. Anything that
does those is a Zetlyn source, whatever it is written in.

The word **must** is used where a thing is not one of these otherwise. Everywhere else the
document says what this implementation does and why, which is a description and not a demand.

## A claim

One source's statement about one thing.

```json
{"claim_id": "…",
 "source": "zetlyn/cve-redhat",
 "kind": "vulnerability",
 "ids": [{"scheme": "cve", "value": "cve-2021-44228"}],
 "title": "log4j: Remote code execution in Log4j 2.x",
 "url": "https://access.redhat.com/security/cve/CVE-2021-44228",
 "text": "…",
 "properties": {"severity": {"code": "important", "vocabulary": "redhat-severity"},
            "cvss": {"number": 10.0},
            "fixed": {"bool": true}},
 "known": "2021-12-10",
 "valid": {"from": "2021-12-10", "to": null},
 "from": {"url": "https://…/cve/CVE-2021-44228.json", "span": null},
 "attachments": [],
 "hash": "sha256:…"}
```

`claim_id` is the SHA-256 of the source name, the kind, and what names the claim. The source
name is in it because a claim is one source's statement: two sources describing the same
vulnerability hold two claims with two ids, which is what makes the join a derivation rather than a
collision.

What names the claim is the identifier where the declaration says a claim carries one, and the
address otherwise. A declaration with `all: true` says its identifiers are references rather than
names: 2,698 Metasploit modules name 2,408 CVEs, several of them exploit one vulnerability, and four
different modules exploit CVE-2021-44228. There the CVE names the thing and the address names the
claim.

An address is a URL, a file, a file and row, or a file and the key a container held the claim under.
A claim that `each` produced gets its own: without one, 2,698 modules would share the address of the
single file they came out of. `hash` is over the identifiers, the title, the text and the
properties, and it is what an update compares to decide whether a claim changed.

`from` is where it came from: a URL, a file and row, or a character span where the claim is part of
a larger document. `attachments` are files the claim points at, each with its media type, its size
and its hash, stored under `blobs/`. An image claim is a claim with one attachment.

### Identifiers

A claim carries one or more, each a scheme and a value. A scheme is a name, a pattern and a
normalisation, declared by the source.

A claim may carry a scheme its own publisher does not issue. A GitHub advisory names the CVE it is
about. A GGUF repository on Hugging Face names the model it quantised. A consolidated act names the
directive it implements. That cross-reference is in the source, and it is what lets two sources
meet.

An identifier is stored as its source wrote it and compared with case folded. `CVE-2021-44228`
and `cve-2021-44228` are one thing; `Qwen/Qwen2.5-32B-Instruct` is a path and lower-casing it
gives a repository that does not exist. Folding on the way in loses the second; not folding on
the way out loses the first. So the value keeps its case and the comparison does not.

There is no table anywhere mapping one identifier to another. Where sources cross-reference each
other the join happens, and where they do not it does not.

### Properties

Six types, and the list is closed.

| type | form |
|---|---|
| `text` | `{"text": "…"}` |
| `code` | `{"code": "important", "vocabulary": "redhat-severity"}` |
| `number` | `{"number": 10.0}` |
| `bool` | `{"bool": true}` |
| `date` | `{"date": "2021-12-10"}` |
| `interval` | `{"interval": {"from": "2021-12-10", "to": null}}` |

A list of any of them is a value. Nothing nests, nothing is conditional, and no property refers to
another. Anything the form cannot hold stays in the text, where a reader sees it as the source wrote
it.

A `code` names its vocabulary rather than standing alone. `important` is Red Hat's word and `high`
is GitHub's, and a tracker maps between them. A bare string would leave a reader guessing which
dialect it is written in.

A value that does not parse as its type does not become a property, and does not quietly vanish
either: the update counts it and names the first three. A source that starts writing `n\/a` into a
date column is visible the same day.

Nothing here orders version strings. `4.17.20` sits between `4.0.0` and `4.17.21` under npm's rules,
not under Debian's and not under RPM's.

### Two dates

| | |
|---|---|
| `known` | when the source said it. Required |
| `valid` | when the statement is true of the world. Optional, open at both ends when absent |

`retention.history` keeps every version, and that is what lets a change say which property moved
and what it moved from. Turned on part-way through a source's life, the first change after
it has nothing to be shown against and says so rather than inventing a previous value.

`as_of` reads one claim as it stood, from those versions. It does not make a whole query
answer as of a date: the text index is the current one. Where no version from on or before
the date is kept, that is what is said.

`known` falls back to the file's modification time, and to the date of the update where there is no
file. The update counts how often it had to.

A provision in force from 2023 to 2026, restated in a consolidation published in 2024, has a `valid`
of that interval and a `known` of the day the consolidation appeared. A question asked about last
year gets last year's answer.

## The interface

A tracker does not read a source's store; it asks, and the questions are the
same six for every source whatever it was built from.

| | |
|---|---|
| `describe()` | what I am, what I hold, what I can be asked |
| `search(query)` | ranked claims matching a query |
| `facet(query, property)` | how many claims per value of a property, under that query |
| `fetch(ids)` | whole claims by identifier or claim id |
| `changes(since, query)` | what was added, changed and removed since a mark |
| `mark()` | the current mark, to be handed back to `changes` later |

### `describe`

The one that carries the weight. Everything a tracker needs to build an overview, a browse and a
search box is in it, and none of it requires touching a claim.

```json
{"source": "zetlyn/cve-redhat", "kind": "vulnerability",
 "title": "Red Hat security advisories",
 "about": "Red Hat's own analysis of a vulnerability, and the state of each affected package.",
 "claims": 41208,
 "state": "current",
 "last_update": {"at": "2026-09-26T07:00:12Z", "complete": true, "added": 31, "changed": 12, "removed": 0},
 "next_update": "2026-09-26T08:00:00Z",

 "schemes": [{"scheme": "cve", "claims": 41208}],

 "properties": [
   {"name": "severity", "type": "code", "vocabulary": "redhat-severity", "claims": 39102,
    "values": [{"value": "important", "claims": 12044}, {"value": "moderate", "claims": 18330}]},
   {"name": "cvss", "type": "number", "claims": 38110, "min": 0.0, "max": 10.0},
   {"name": "fixed", "type": "bool", "claims": 41208, "values": [{"value": true, "claims": 27004}]}],

 "vocabulary": {"redhat-severity": {"important": "A flaw that can easily compromise…"}},

 "views": [{"name": "recent", "title": "Newest first", "default": true},
           {"name": "unfixed", "title": "Not fixed yet"},
           {"name": "by-severity", "title": "By severity", "group": "severity"}],

 "search": {"text": ["title", "text"], "compare": ["cvss", "known"],
            "suggest": ["packages"],
            "examples": ["log4j", "severity=important and fixed=false", "CVE-2021-44228"]},

 "can": ["text", "property", "facet", "ids", "changes", "as_of"]}
```

Per property: its type, how many claims carry it, and for a `code` or a `bool` every value with its
count, for a `number` or a `date` its range. That is a faceted browse before a query has been asked,
and it is what makes a tracker's overview instant over sources of unlike shape.

### `can`

Not every source does everything. A CSV of 200 rows has no vector index. A source that reads a
live SQL view cannot answer `as_of`, because it holds no history. A third party's source reached
over HTTP may answer `search` and nothing else.

A tracker asks what each source can do and composes what it gets. A source that cannot answer part
of a query says so, and the tracker shows which sources answered it and which could not:
`severity>=high — answered by 4 of 6 sources, 2 carry no severity`, above the results.

A filter quietly applied to four sources out of six is a wrong count presented as a right one.

### `search`

```json
{"text": "log4j",
 "filter": [["severity", "gte", "high"], ["known", "gt", "2026-01-01"]],
 "ids": [{"scheme": "cve", "value": "cve-2021-44228"}],
 "view": "unfixed",
 "sort": "known desc",
 "limit": 50, "cursor": "…"}
```

A hit carries its rank within this source's answer and never a score.

```json
{"claim_id": "…", "rank": 3,
 "why": {"text": ["log4j", "jndi"], "property": ["severity"], "id": null},
 "title": "…", "url": "…", "ids": [], "properties": {}}
```

Two sources index different bodies of text of different sizes with different vocabularies, and their
scores are not on one scale. Putting them on one would mean inventing a conversion nobody can check.
So the tracker merges ranked lists rather than scored ones, takes the best remaining hit from each
source in turn, and breaks a tie with the source's `priority`. The order is visible without a
number, and `priority` is the only thing that moved it.

`why` travels with the hit because the source is the only thing that knows it. A tracker that had to
explain a match would be reimplementing every source's retrieval.

### Where the interface runs

In process, as a Rust trait, for every source this workspace holds. Over HTTP, with the same six
calls and the same shapes, for a source somewhere else. One interface and not two, which is what
makes a remote source ordinary rather than a feature, and what M7 in [PLAN.md](PLAN.md) needs.

It is written now rather than then because a tracker built to read its sources' stores directly
would have to be taken apart to reach the first source that is not ours.

### Sharing

A source belongs to no tracker. Several trackers may name the same source, they ask it the same six
questions, and it answers all of them identically. A source has no idea how many trackers it is in,
and the cost of a source is paid by whoever fetches it rather than by each tracker again.


## The hub

A catalogue is one workspace's. A hub is where a source somebody else operates is fetched from:
a directory layout served over HTTPS and nothing more. Any web server or object store is one, and
a mirror is a file copy.

`hub.zetlyn.com` is one hub. It is not privileged here; it is the default when a reference names no
host.

### What travels

A source travels as bytes. A tracker travels as a statement.

The publisher updates the source and ships what came out. A subscriber holds claims, not
instructions for producing them, and therefore needs none of the publisher's credentials, is not
subject to the source's rate limits, and does not add a caller to a source that has one publisher
and could have had a thousand. `zetlyn/models-hf` is the case: unauthenticated, it is throttled off
after 578 to 734 of 2,684 things, and every subscriber running it themselves would find that out
separately.

A tracker has no payload because it has no claims. Its sources are sources, it holds no index, and
what it publishes is the statement: which sources, what joins them, what the words mean, what it
promises. Shipping the sources' bytes inside it would ship the same source once per tracker that
names it, and a source belongs to no tracker.

### A reference

```
[host/]owner/name[@tag]
```

```
zetlyn/cve-kev                           the default hub
zetlyn/cve-kev@2026-09                   a tag
hub.example.com/zetlyn/cve-kev@2026-09   another hub
```

The first segment is a host when it contains a dot, which is why an owner may not contain one. A
reference with no host resolves `hub.zetlyn.com`, and every command that takes a hub takes it that
way: `--from` and `--to` are for a folder, a mount, a bucket or another hub, and naming the default
in one of them says the same thing twice. A reference with no tag resolves `latest`, which is an
ordinary tag a publisher sets and not an automatic one; a publisher who sets none has no untagged
reference.

A reference is not the source's name. Two hubs may serve `zetlyn/cve-kev` and the manifests inside
carry the same name, which one workspace cannot hold twice. A subscriber who wants both renames one
locally: the name in the artifact is the publisher's, the name in a workspace is the operator's, and
the host in a reference says where the bytes came from.

### The artifact is not the store

A store holds more than the claims. `zetlyn/cve-kev` holds 1,726 claims, 11,907 properties and 1,726
identifiers, and alongside them 5,180 revisions, twelve updates with their errors and refusals, and
six tables of full-text index. A source whose fetch takes a watermark keeps that here too.

The revisions, the updates and the watermark are the publisher's operating history. The full-text
index is derived from the text and rebuilt on arrival. None of it travels, and the difference is
not small: over the eleven sources here, 98,546 claims, the stores are 398 MB and the claims
themselves are 61 MB.

| | |
|---|---|
| travels | the claims: identity, kind, title, url, text, known, identifiers, typed properties, hash |
| rebuilt on arrival | the full-text index, the identifier index, the property index |
| stays with the publisher | the revision history, the update history, the watermarks, the secrets |

A subscriber who wants history keeps their own, built from the versions they have applied. It
begins the day they subscribed and not the day the publisher started.

### The manifest

One JSON object per published version. It travels inside the artifact rather than beside it, so a
manifest that arrives by a copied key or a mis-synced bucket still says what the payloads are.

#### Identity

| | |
|---|---|
| `spec_version` | the version of this specification the artifact was built against |
| `source` | the publisher's name for it, as the declaration carries it: `zetlyn/cve-kev` |
| `version` | content hash of the payloads this manifest covers |
| `applies_to` | on a delta, the one version it applies to. Absent on a full version |
| `built_at` | unix seconds, supplied by the builder and never read from a clock, so a build is reproducible |
| `built_by` | what made it, as a name and a version |
| `signed_by` | whose signature to expect in `manifest.sig` |
| `kind` | what one claim is |
| `title`, `about` | the publisher's own words, shown wherever the source is |

#### What it holds

| | |
|---|---|
| `claims` | the count |
| `identifiers` | per scheme, the number of distinct values. A tracker joins on a scheme, and this is how a curator sees whether it can, before fetching anything |
| `properties` | per property: name, type, how many claims carry it, and for a coded one its vocabulary |
| `known` | the first and last `known` date in it, which is the coverage in time |

`properties` is the same answer `describe` gives, and it is in the manifest because the question it
answers — is this source any use to me — should not cost 61 MB.

#### The update behind it

| | |
|---|---|
| `complete` | whether the update that produced these bytes saw the whole source |
| `reached` | on a partial update, what it got to, in the publisher's own words |
| `finished` | when that update ended |
| `every` | the cadence the publisher intends to publish at, so a subscriber can pace their own pull |

`covers` and `excludes` are a tracker's and not a source's. What a source covers is its `about`,
the span of its `known`, and whether the update that built it was complete.

A subscriber inherits the publisher's state. If the update that built these bytes reached 578 of
2,684 things, every subscriber holds those 578, and a tracker that names this source is partial for
the same reason its publisher's is. That fact travels in the artifact rather than living on the
publisher's front page, because the subscriber's readers never see that page.

#### Terms

| | |
|---|---|
| `fetched_from` | where the publisher fetched it from |
| `text_is` | `summary` or `whole`: whether the text in these claims is what the source published about itself, or the thing itself |
| `terms` | prose, the publisher's statement of what a subscriber may do with these bytes |

Fetching, indexing and republishing are three acts and a source may permit one and not the next.
When bytes travel, the publisher performs the third on the subscriber's behalf, so the licence
decision is theirs and `text_is` is where it is visible in one line. `terms` is not checked by
anything. It is the claim a person makes and answers for.

`text_is` travels into the subscribed declaration as well as sitting in the manifest, because a
subscriber holding these claims has to answer for them too.

#### How it is read

| | |
|---|---|
| `read.views` | every view the source declares: its columns, its facets, its sort, its filter |
| `read.search` | which parts the index covers, which properties compare, what to suggest, and the examples |

A source says how it wants to be read, and a tracker adopts what a source declares rather than
inventing a second opinion about somebody else's data. A subscriber who lost that would hold a
worse thing than the publisher does, so it travels.

The examples are the part worth having. They are queries the publisher claims return something,
and `zetlyn source check` holds them against the claims that arrived.

#### Payloads

```json
"payloads": {
  "claims.jsonl": {"bytes": 19173376, "sha256": "…"}
}
```

Every file this version consists of, with its size and its hash. A subscriber fetches what the
manifest lists and checks each file against it; anything else served under this version is not part
of it. A payload may be compressed, in which case its name carries the suffix and the manifest
lists that name.

`claims.jsonl` is one claim per line, in the shape section *A claim* defines, each carrying the
hash it already has. On a delta the payloads are what the delta ships: the claims that were added
or changed, and `removed.jsonl`, one claim id per line.

### Who published it

A hash per payload says the bytes are the ones this manifest describes. It does not say who wrote
the manifest, and it cannot: whoever serves the payload serves the manifest beside it, so a hub
that wanted to hand somebody different claims would write both and the hashes would agree.

`manifest.sig` beside `manifest.json` is an ed25519 signature over the manifest exactly as served,
and the manifest's `signed_by` says whose. A tracker carries both too: its manifest is the
composition whole, so whoever can change it can change which sources a subscriber assembles and
what their words are taken to mean.

A subscriber pins the key in their own declaration:

```yaml
fetch:
  type: hub
  at: https://hub.zetlyn.com
  ref: zetlyn/cve-kev@latest
  key: ed25519:7d05a945…
```

Where a key is pinned, a version whose manifest is not signed by it is not applied, and that
includes a version with no signature at all.

Where none is pinned, the manifest's own `signed_by` is used and written into the declaration, so
every fetch after the first is held against the first. On the first there is nothing to catch a
hub that wrote both halves of the claim, and that is what pinning by hand is for. A subscriber who
took a tracker gets each source pinned this way, because they never named those keys themselves.

One key per source, and publishing the same source from a second machine under a second key
stops every subscriber who holds the first. That is the cost of the guarantee and not a fault in
it: a publisher is a key, and two keys are two publishers.

Signing is the only thing here a hub cannot do for a publisher, and it is the reason a hub can be
a directory over HTTPS and nothing more. Everything else a hub holds, a hub could have written.

### A delta

A source that updates hourly from a place where one row changed cannot ask every subscriber for the
whole of it.

A version directory is written once and never changed, and it always holds the whole source. A
delta sits inside the version it produces, under the version it applies to:

```
/sources/{owner}/{name}/versions/{v}/manifest.json
/sources/{owner}/{name}/versions/{v}/claims.jsonl          the whole of it
/sources/{owner}/{name}/versions/{v}/from/{p}/manifest.json
/sources/{owner}/{name}/versions/{v}/from/{p}/claims.jsonl the claims that arrived or changed
/sources/{owner}/{name}/versions/{v}/from/{p}/removed.jsonl one claim id per line
```

The directory names what a subscriber holds *after* applying it, so every hash in `tags/` and every
hash a subscriber pins is a source and never a transport. `from/{p}` names what to apply it to.

A subscriber holding `p` asks for `from/{p}`. Where it is not there, or is not smaller than the
whole, they take the whole, and the two give the same store: the delta carries every claim that
arrived or changed and every identifier that left, and nothing else moved.

A subscriber holding nothing takes one version and applies no chain. That is the reason the full
payloads stay beside the delta rather than being replaced by it. It costs the publisher a copy of
the claims per version, which is the cheaper of the two mistakes: the other one makes a first
subscription walk a chain from whenever the publisher started.

Several `from/` may sit under one version, because a subscriber two versions behind is not
unusual. A publisher who keeps only the newest is right most of the time, and a subscriber who
finds no delta for the version they hold takes the whole and is not told twice.

### A tracker on the hub

No payload. Its manifest carries the tracker declaration whole — the sources, the identifiers, the
alignment, the views, the promise — and its version is the content hash of that declaration rather
than of the manifest around it. The manifest carries a build time, so hashing it would make every
republication a new version and lose the thing a version is for: two publications of the same
composition are one version.

Sources are named by reference without a version, so a tracker follows each source's `latest` and
stays current as they publish. The manifest also claims which versions the curator last checked it
against, which is what `zetlyn tracker check` was run over. That is information and not a pin: a
subscriber assembling the tracker from newer sources gets newer sources, and a curator who wants
otherwise names a tag.

Subscribing to a tracker takes the statement and then each source it names that is not held already,
from the same hub. A source named as a remote is left alone: it is somebody's live surface and
there is nothing to fetch.

### Who may write

A folder, a mount and somebody's own bucket need nobody's permission: whoever can write there may,
and there is no name to contend for. The rule below is for the one case that is different, which
is a hub several people publish to.

An owner name is registered, first come and for good, and it allows writing under
`sources/{owner}/` and `trackers/{owner}/` and nowhere else. What it belongs to is a key. Nothing
is handed out at registration, because the person registering already holds the half that signs
and the hub is only being told which key that is. A copy of the owners file says who publishes
here and lets the reader do nothing.

A write is signed, over the method, the path, the body and the time, the same way a console call
is. A hub anybody may read therefore gives nothing away by being read, and a signature cannot be
lifted into a different request the way a token can be copied into one.

Some names are not available to anybody. Three kinds, and each is somebody being deceived rather
than inconvenienced: a name that would read as this project speaking, a name the layout already
uses so that a reference could not tell it from a tree, and the name of a body the publisher is
not. `cve` is not available; `cve-mirror` is.

A name is lower-case letters, digits and hyphens, two to thirty-nine characters, no hyphen at
either end and no two in a row. No dots, because the first segment of a reference is a host when
it has one.

One key holds one name on one hub, and asking for a second under a key that already has one is
refused. A person who wants two names has two keys and is two publishers, which is what they
would look like to a subscriber anyway.

### What a hub carries

Two addresses beyond the layout, and both are about the hub rather than about anything in it.

| | |
|---|---|
| `/index.json` | one thing per tag the hub carries: tree, reference, tag, version, title, about, `built_at`, and the claim count and payload size the manifest states |
| `/` | the same list as a page |

Both are built by reading each tag and the manifest it names. No claim is opened.

### What a hub is not

It does not answer queries, hold a tracker's index, or know what a subscriber does with a source. It
serves files. A source fetched from a hub is read locally, and no question ever reaches the hub.

A host may serve other things under the same name. `hub.zetlyn.com` carries the bytes and also
serves two trackers, at `/zetlyn/cve` and `/zetlyn/local-models`. Those are workspaces: each holds
its own store, answers its own queries and has its own readers, and a query reaches one of them and
never the hub. The paths do not collide because the hub's own trees are `sources/` and `trackers/`,
which no owner name may be.

### Freshness at a subscriber

A subscriber's update is a fetch. Stamping it with the moment of the fetch would make a year-old
version look as fresh as the minute it arrived, so a subscribed update carries the time the
publisher's update finished, which the manifest states. A tracker's `fresh_within` is therefore
about the age of the claims and not about when somebody last downloaded them.

## What changed from 1.0

The words, and nothing else. What a claim holds, the six calls and what they answer, and what a
hub carries are the same as they were; each has another name.

| 1.0 | 2.0 |
|---|---|
| a record | a claim: `claim_id` for `record_id`, `source` for `dataset`, `properties` for `fields` |
| a dataset | a source: `describe` says `source`, `claims`, `properties`, `last_update`, `next_update` |
| a scope, its members, an entry | a tracker, its sources, a thing: `/thing/{scheme}/{value}` for `/entry/…` |
| `divergent` | `conflict` |
| `can` names `field` | it names `property` |
| `facet(query, field)` | `facet(query, property)`: `/api/facet?property=…` |
| `datasets/` and `scopes/` on a hub | `sources/` and `trackers/` |
| `records.jsonl` | `claims.jsonl` |
| a manifest's `dataset`, `records`, `fields`, `source` | `source`, `claims`, `properties`, `fetched_from` |
| a tracker manifest's `scope`, `members`, `join` | `tracker`, `sources`, `identified_by` |

A 1.0 artifact is refused by a 2.0 subscriber with the version it was built against, rather than
read under names it does not carry.
