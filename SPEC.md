# The Zetlyn specifications

Version 1.0.

Three contracts, and nothing else here is one. The rest of how Zetlyn works is documentation: read
`README.md`, or read the code, which is the same length and more honest.

| | |
|---|---|
| [A record](#a-record) | what a dataset produces |
| [The interface](#the-interface) | the six calls it answers |
| [The hub](#the-hub) | how a dataset travels, and what the manifest says |

A dataset somebody else operates has to satisfy the first two to be a member of a scope. An
artifact somebody else builds has to satisfy the third to be fetched from a hub. Anything that
does those is a Zetlyn dataset, whatever it is written in.

The word **must** is used where a thing is not one of these otherwise. Everywhere else the
document says what this implementation does and why, which is a description and not a demand.

## A record

One dataset's statement about one thing.

```json
{"record_id": "…",
 "dataset": "zetlyn/cve-redhat",
 "kind": "vulnerability",
 "ids": [{"scheme": "cve", "value": "cve-2021-44228"}],
 "title": "log4j: Remote code execution in Log4j 2.x",
 "url": "https://access.redhat.com/security/cve/CVE-2021-44228",
 "text": "…",
 "fields": {"severity": {"code": "important", "vocabulary": "redhat-severity"},
            "cvss": {"number": 10.0},
            "fixed": {"bool": true}},
 "known": "2021-12-10",
 "valid": {"from": "2021-12-10", "to": null},
 "from": {"url": "https://…/cve/CVE-2021-44228.json", "span": null},
 "attachments": [],
 "hash": "sha256:…"}
```

`record_id` is the SHA-256 of the dataset name, the kind, and what names the record. The dataset
name is in it because a record is one dataset's statement: two datasets describing the same
vulnerability hold two records with two ids, which is what makes the join a derivation rather than a
collision.

What names the record is the identifier where the declaration says a record carries one, and the
address otherwise. A declaration with `all = true` says its identifiers are references rather than
names: 2,698 Metasploit modules name 2,408 CVEs, several of them exploit one vulnerability, and four
different modules exploit CVE-2021-44228. There the CVE names the subject and the address names the
record.

An address is a URL, a file, a file and row, or a file and the key a container held the record
under. A record that `each` produced gets its own: without one, 2,698 modules would share the
address of the single file they came out of.
`hash` is over the identifiers, the title, the text and the fields, and it is what a run compares to
decide whether a record changed.

`from` is where it came from: a URL, a file and row, or a character span where the record is part of
a larger document. `attachments` are files the record points at, each with its media type, its size
and its hash, stored under `blobs/`. An image record is a record with one attachment.

### Identifiers

A record carries one or more, each a scheme and a value. A scheme is a name, a pattern and a
normalisation, declared by the dataset.

A record may carry a scheme its own publisher does not issue. A GitHub advisory names the CVE it is
about. A GGUF repository on Hugging Face names the model it quantised. A consolidated act names the
directive it implements. That cross-reference is in the source, and it is what lets two datasets
meet.

An identifier is stored as its source wrote it and compared with case folded. `CVE-2021-44228`
and `cve-2021-44228` are one thing; `Qwen/Qwen2.5-32B-Instruct` is a path and lower-casing it
gives a repository that does not exist. Folding on the way in loses the second; not folding on
the way out loses the first. So the value keeps its case and the comparison does not.

There is no table anywhere mapping one identifier to another. Where sources cross-reference each
other the join happens, and where they do not it does not.

### Fields

Six types, and the list is closed.

| type | form |
|---|---|
| `text` | `{"text": "…"}` |
| `code` | `{"code": "important", "vocabulary": "redhat-severity"}` |
| `number` | `{"number": 10.0}` |
| `bool` | `{"bool": true}` |
| `date` | `{"date": "2021-12-10"}` |
| `interval` | `{"interval": {"from": "2021-12-10", "to": null}}` |

A list of any of them is a value. Nothing nests, nothing is conditional, and no field refers to
another. Anything the form cannot hold stays in the text, where a reader sees it as the source wrote
it.

A `code` names its vocabulary rather than standing alone. `important` is Red Hat's word and `high` is
GitHub's, and a scope maps between them. A bare string would leave a reader guessing which dialect it
is written in.

A value that does not parse as its type does not become a field, and does not quietly vanish
either: the run counts it and names the first three. A source that starts writing `n\/a` into a
date column is visible the same day.

Nothing here orders version strings. `4.17.20` sits between `4.0.0` and `4.17.21` under npm's rules,
not under Debian's and not under RPM's.

### Two dates

| | |
|---|---|
| `known` | when the source said it. Required |
| `valid` | when the statement is true of the world. Optional, open at both ends when absent |

`retention.history` keeps every version, and that is what lets a change say which field moved
and what it moved from. Turned on part-way through a dataset's life, the first change after
it has nothing to be shown against and says so rather than inventing a previous value.

`as_of` reads one record as it stood, from those versions. It does not make a whole query
answer as of a date: the text index is the current one. Where no version from on or before
the date is kept, that is what is said.

`known` falls back to the file's modification time, and to the date of the run where there is no
file. The run counts how often it had to.

A provision in force from 2023 to 2026, restated in a consolidation published in 2024, has a `valid`
of that interval and a `known` of the day the consolidation appeared. A question asked about last
year gets last year's answer.

## The interface

A scope does not read a member's store; it asks, and the questions are the
same six for every dataset whatever it was built from.

| | |
|---|---|
| `describe()` | what I am, what I hold, what I can be asked |
| `search(query)` | ranked records matching a query |
| `facet(query, field)` | how many records per value of a field, under that query |
| `fetch(ids)` | whole records by identifier or record id |
| `changes(since, query)` | what was added, changed and removed since a mark |
| `mark()` | the current mark, to be handed back to `changes` later |

### `describe`

The one that carries the weight. Everything a scope needs to build an overview, a browse and a search
box is in it, and none of it requires touching a record.

```json
{"dataset": "zetlyn/cve-redhat", "kind": "vulnerability",
 "title": "Red Hat security advisories",
 "about": "Red Hat's own analysis of a vulnerability, and the state of each affected package.",
 "records": 41208,
 "state": "current",
 "last_run": {"at": "2026-09-26T07:00:12Z", "complete": true, "added": 31, "changed": 12, "removed": 0},
 "next_run": "2026-09-26T08:00:00Z",

 "schemes": [{"scheme": "cve", "records": 41208}],

 "fields": [
   {"name": "severity", "type": "code", "vocabulary": "redhat-severity", "records": 39102,
    "values": [{"value": "important", "records": 12044}, {"value": "moderate", "records": 18330}]},
   {"name": "cvss", "type": "number", "records": 38110, "min": 0.0, "max": 10.0},
   {"name": "fixed", "type": "bool", "records": 41208, "values": [{"value": true, "records": 27004}]}],

 "vocabulary": {"redhat-severity": {"important": "A flaw that can easily compromise…"}},

 "views": [{"name": "recent", "title": "Newest first", "default": true},
           {"name": "unfixed", "title": "Not fixed yet"},
           {"name": "by-severity", "title": "By severity", "group": "severity"}],

 "search": {"text": ["title", "text"], "compare": ["cvss", "known"],
            "suggest": ["packages"],
            "examples": ["log4j", "severity=important and fixed=false", "CVE-2021-44228"]},

 "can": ["text", "field", "facet", "ids", "changes", "as_of"]}
```

Per field: its type, how many records carry it, and for a `code` or a `bool` every value with its
count, for a `number` or a `date` its range. That is a faceted browse before a query has been asked,
and it is what makes a scope's overview instant over members of unlike shape.

### `can`

Not every dataset does everything. A CSV of 200 rows has no vector index. A dataset whose source is a
live SQL view cannot answer `as_of`, because it holds no history. A third party's dataset reached
over HTTP may answer `search` and nothing else.

A scope asks what each member can do and composes what it gets. A member that cannot answer part of a
query says so, and the scope shows which members answered it and which could not: `severity>=high —
answered by 4 of 6 members, 2 carry no severity`, above the results.

A filter quietly applied to four members out of six is a wrong count presented as a right one.

### `search`

```json
{"text": "log4j",
 "filter": [["severity", "gte", "high"], ["known", "gt", "2026-01-01"]],
 "ids": [{"scheme": "cve", "value": "cve-2021-44228"}],
 "view": "unfixed",
 "sort": "known desc",
 "limit": 50, "cursor": "…"}
```

A hit carries its rank within this member's answer and never a score.

```json
{"record_id": "…", "rank": 3,
 "why": {"text": ["log4j", "jndi"], "field": ["severity"], "id": null},
 "title": "…", "url": "…", "ids": [], "fields": {}}
```

Two members index different bodies of text of different sizes with different vocabularies, and their
scores are not on one scale. Putting them on one would mean inventing a conversion nobody can check.
So the scope merges ranked lists rather than scored ones, takes the best remaining hit from each
member in turn, and breaks a tie with the member's `priority`. The order is visible without a number,
and `priority` is the only thing that moved it.

`why` travels with the hit because the member is the only thing that knows it. A scope that had to
explain a match would be reimplementing every member's retrieval.

### Where the interface runs

In process, as a Rust trait, for every dataset this deployment holds. Over HTTP, with the same six
calls and the same shapes, for a dataset somewhere else. One interface and not two, which is what
makes a remote dataset ordinary rather than a feature, and what M7 in [PLAN.md](PLAN.md) needs.

It is written now rather than then because a scope built to read its members' stores directly would
have to be taken apart to reach the first dataset that is not ours.

### Sharing

A dataset belongs to no scope. Several scopes may name the same member, they ask it the same six
questions, and it answers all of them identically. A member has no idea how many scopes it is in, and
the cost of a source is paid by whoever fetches it rather than by each scope again.


## The hub

A catalogue is one deployment's. A hub is where a dataset somebody else operates is fetched from:
a directory layout served over HTTPS and nothing more. Any web server or object store is one, and
a mirror is a file copy.

`hub.zetlyn.com` is one hub. It is not privileged here; it is the default when a reference names no
host.

### What travels

A dataset travels as bytes. A scope travels as a statement.

The publisher runs the source and ships what came out. A subscriber holds records, not instructions
for producing them, and therefore needs none of the publisher's credentials, is not subject to the
source's rate limits, and does not add a caller to a source that has one publisher and could have
had a thousand. `zetlyn/models-hf` is the case: unauthenticated, it is throttled off after 578 to 734 of
2,684 subjects, and every subscriber running it themselves would find that out separately.

A scope has no payload because it has no records. Its members are datasets, it holds no index, and
what it publishes is the statement: which datasets, what joins them, what the words mean, what it
promises. Shipping the members' bytes inside it would ship the same dataset once per scope that
names it, and a dataset belongs to no scope.

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

A reference is not the dataset's name. Two hubs may serve `zetlyn/cve-kev` and the manifests inside carry
the same name, which one deployment cannot hold twice. A subscriber who wants both renames one
locally: the name in the artifact is the publisher's, the name in a deployment is the operator's,
and the host in a reference says where the bytes came from.

### The artifact is not the store

A store holds more than the records. `zetlyn/cve-kev` holds 1,726 records, 11,907 fields and 1,726
identifiers, and alongside them 5,180 revisions, twelve runs with their errors and refusals, and
six tables of full-text index. A dataset whose source takes a watermark keeps that here too.

The revisions, the runs and the watermark are the publisher's operating history. The full-text
index is derived from the text and rebuilt on arrival. None of it travels, and the difference is
not small: over the eleven datasets here, 98,546 records, the stores are 398 MB and the records
themselves are 61 MB.

| | |
|---|---|
| travels | the records: identity, kind, title, url, text, known, identifiers, typed fields, hash |
| rebuilt on arrival | the full-text index, the identifier index, the field index |
| stays with the publisher | the revision history, the run history, the watermarks, the secrets |

A subscriber who wants history keeps their own, built from the versions they have applied. It
begins the day they subscribed and not the day the publisher started.

### The manifest

One JSON object per published version. It travels inside the artifact rather than beside it, so a
manifest that arrives by a copied key or a mis-synced bucket still says what the payloads are.

#### Identity

| | |
|---|---|
| `spec_version` | the version of this specification the artifact was built against |
| `dataset` | the publisher's name for it, as the declaration carries it: `zetlyn/cve-kev` |
| `version` | content hash of the payloads this manifest covers |
| `applies_to` | on a delta, the one version it applies to. Absent on a full version |
| `built_at` | unix seconds, supplied by the builder and never read from a clock, so a build is reproducible |
| `built_by` | what made it, as a name and a version |
| `signed_by` | whose signature to expect in `manifest.sig` |
| `kind` | what one record is |
| `title`, `about` | the publisher's own words, shown wherever the dataset is |

#### What it holds

| | |
|---|---|
| `records` | the count |
| `identifiers` | per scheme, the number of distinct values. A scope joins on a scheme, and this is how a curator sees whether it can, before fetching anything |
| `fields` | per field: name, type, how many records carry it, and for a coded field its vocabulary |
| `known` | the first and last `known` date in it, which is the coverage in time |

`fields` is the same answer `describe` gives, and it is in the manifest because the question it
answers — is this dataset any use to me — should not cost 61 MB.

#### The run behind it

| | |
|---|---|
| `complete` | whether the run that produced these bytes saw the whole source |
| `reached` | on a partial run, what it got to, in the publisher's own words |
| `finished` | when that run ended |
| `every` | the cadence the publisher intends to publish at, so a subscriber can pace their own pull |

`covers` and `excludes` are a scope's and not a dataset's. What a dataset covers is its `about`,
the span of its `known`, and whether the run that built it was complete.

A subscriber inherits the publisher's state. If the run that built these bytes reached 578 of 2,684
subjects, every subscriber holds those 578, and a scope that names this dataset is partial for the
same reason its publisher's is. That fact travels in the artifact rather than living on the
publisher's front page, because the subscriber's readers never see that page.

#### Terms

| | |
|---|---|
| `source` | where the publisher fetched it from |
| `text_is` | `summary` or `whole`: whether the text in these records is what the source published about itself, or the thing itself |
| `terms` | prose, the publisher's statement of what a subscriber may do with these bytes |

Fetching, indexing and republishing are three acts and a source may permit one and not the next.
When bytes travel, the publisher performs the third on the subscriber's behalf, so the licence
decision is theirs and `text_is` is where it is visible in one line. `terms` is not checked by
anything. It is the claim a person makes and answers for.

`text_is` travels into the subscribed declaration as well as sitting in the manifest, because a
subscriber holding these records has to answer for them too.

#### How it is read

| | |
|---|---|
| `read.views` | every view the dataset declares: its columns, its facets, its sort, its filter |
| `read.search` | which parts the index covers, which fields compare, what to suggest, and the examples |

A dataset says how it wants to be read, and a scope adopts what a member declares rather than
inventing a second opinion about somebody else's data. A subscriber who lost that would hold a
worse thing than the publisher does, so it travels.

The examples are the part worth having. They are queries the publisher claims return something,
and `zetlyn dataset check` holds them against the records that arrived.

#### Payloads

```json
"payloads": {
  "records.jsonl": {"bytes": 19173376, "sha256": "…"}
}
```

Every file this version consists of, with its size and its hash. A subscriber fetches what the
manifest lists and checks each file against it; anything else served under this version is not part
of it. A payload may be compressed, in which case its name carries the suffix and the manifest
lists that name.

`records.jsonl` is one record per line, in the shape section *A record* defines, each carrying the
hash it already has. On a delta the payloads are what the delta ships: the records that were added
or changed, and `removed.jsonl`, one record id per line.

### Who published it

A hash per payload says the bytes are the ones this manifest describes. It does not say who wrote
the manifest, and it cannot: whoever serves the payload serves the manifest beside it, so a hub
that wanted to hand somebody different records would write both and the hashes would agree.

`manifest.sig` beside `manifest.json` is an ed25519 signature over the manifest exactly as served,
and the manifest's `signed_by` says whose. A scope carries both too: its manifest is the
composition whole, so whoever can change it can change which datasets a subscriber assembles and
what their words are taken to mean.

A subscriber pins the key in their own declaration:

```toml
[source]
type = "hub"
at   = "https://hub.zetlyn.com"
ref  = "zetlyn/cve-kev@latest"
key  = "ed25519:7d05a945…"
```

Where a key is pinned, a version whose manifest is not signed by it is not applied, and that
includes a version with no signature at all.

Where none is pinned, the manifest's own `signed_by` is used and written into the declaration, so
every fetch after the first is held against the first. On the first there is nothing to catch a
hub that wrote both halves of the claim, and that is what pinning by hand is for. A subscriber who
took a scope gets each member pinned this way, because they never named those keys themselves.

One key per dataset, and publishing the same dataset from a second machine under a second key
stops every subscriber who holds the first. That is the cost of the guarantee and not a fault in
it: a publisher is a key, and two keys are two publishers.

Signing is the only thing here a hub cannot do for a publisher, and it is the reason a hub can be
a directory over HTTPS and nothing more. Everything else a hub holds, a hub could have written.

### A delta

A dataset that runs hourly over a source where one row changed cannot ask every subscriber for the
whole of it.

A version directory is written once and never changed, and it always holds the whole dataset. A
delta sits inside the version it produces, under the version it applies to:

```
/datasets/{owner}/{name}/versions/{v}/manifest.json
/datasets/{owner}/{name}/versions/{v}/records.jsonl          the whole of it
/datasets/{owner}/{name}/versions/{v}/from/{p}/manifest.json
/datasets/{owner}/{name}/versions/{v}/from/{p}/records.jsonl the records that arrived or changed
/datasets/{owner}/{name}/versions/{v}/from/{p}/removed.jsonl one record id per line
```

The directory names what a subscriber holds *after* applying it, so every hash in `tags/` and every
hash a subscriber pins is a dataset and never a transport. `from/{p}` names what to apply it to.

A subscriber holding `p` asks for `from/{p}`. Where it is not there, or is not smaller than the
whole, they take the whole, and the two give the same store: the delta carries every record that
arrived or changed and every identifier that left, and nothing else moved.

A subscriber holding nothing takes one version and applies no chain. That is the reason the full
payloads stay beside the delta rather than being replaced by it. It costs the publisher a copy of
the records per version, which is the cheaper of the two mistakes: the other one makes a first
subscription walk a chain from whenever the publisher started.

Several `from/` may sit under one version, because a subscriber two versions behind is not
unusual. A publisher who keeps only the newest is right most of the time, and a subscriber who
finds no delta for the version they hold takes the whole and is not told twice.

### A scope on the hub

No payload. Its manifest carries the scope declaration whole — the members, the join, the maps,
the views, the promise — and its version is the content hash of that declaration rather than of
the manifest around it. The manifest carries a build time, so hashing it would make every
republication a new version and lose the thing a version is for: two publications of the same
composition are one version.

Members are named by reference without a version, so a scope follows each member's `latest` and
stays current as they publish. The manifest also records which versions the curator last checked it
against, which is what `zetlyn scope check` was run over. That is information and not a pin: a
subscriber assembling the scope from newer members gets newer members, and a curator who wants
otherwise names a tag.

Subscribing to a scope takes the statement and then each member it names that is not held already,
from the same hub. A member named as a remote is left alone: it is somebody's live surface and
there is nothing to fetch.

### Who may write

A folder, a mount and somebody's own bucket need nobody's permission: whoever can write there may,
and there is no name to contend for. The rule below is for the one case that is different, which
is a hub several people publish to.

An owner name is registered, first come and for good, and it allows writing under
`datasets/{owner}/` and `scopes/{owner}/` and nowhere else. What it belongs to is a key. Nothing
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
| `/index.json` | one entry per tag the hub carries: tree, reference, tag, version, title, about, `built_at`, and the record count and payload size the manifest states |
| `/` | the same list as a page |

Both are built by reading each tag and the manifest it names. No record is opened.

### What a hub is not

It does not answer queries, hold a scope's index, or know what a subscriber does with a dataset. It
serves files. A dataset fetched from a hub is read locally, and no question ever reaches the hub.

A host may serve other things under the same name. `hub.zetlyn.com` carries the bytes and also
serves two scopes, at `/zetlyn/cve` and `/zetlyn/local-models`. Those are deployments: each holds
its own store, answers its own queries and has its own readers, and a query reaches one of them and
never the hub. The paths do not collide because the hub's own trees are `datasets/` and `scopes/`,
which no owner name may be.

### Freshness at a subscriber

A subscriber's run is a fetch. Stamping it with the moment of the fetch would make a year-old
version look as fresh as the minute it arrived, so a subscribed run carries the time the
publisher's run finished, which the manifest states. A scope's `fresh_within` is therefore about
the age of the records and not about when somebody last downloaded them.
