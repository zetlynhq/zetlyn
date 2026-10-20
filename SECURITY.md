# Security

Please report a vulnerability privately to **hello@zetlyn.com** (subject "Security") rather than in a public issue.
Say what you found and how to see it; you will hear back within a few days. zetlyn.com runs the
latest release, so a fix there reaches everyone who uses the hosted service.

Zetlyn fetches addresses strangers give it, so the line it must hold is clear: it never reaches
loopback, private networks or a cloud's metadata service (`src/outbound.rs`), whatever an address
resolves to or redirects to, and it renders pages in a Chrome that loads nothing private either.
A way around that is the most welcome report.
