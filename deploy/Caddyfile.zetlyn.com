# /etc/caddy/Caddyfile on n1: zetlyn.com is Zetlyn (zetlynhq/zetlyn), a URL in, its changes out.
# Every address is the app on 127.0.0.1:4848 (zetlyn.service), the legal pages and llms.txt
# included. Only the old product's addresses are answered here, by a redirect. No subdomains; www
# only redirects here.
{
	email {$ZETLYN_ACME_EMAIL}
}

zetlyn.com {
	encode zstd gzip

	# One page, one address: the forms that name a file go to the form that names a page.
	@page path_regexp pg ^(.*)\.html$
	redir @page {re.pg.1} permanent
	redir /impressum /legal permanent
	redir /datenschutz /privacy permanent
	redir /favicon.ico /favicon.png permanent

	# The old product's addresses go to the front page: 302 while the change is new.
	@old path /hub /hub/* /hosting /order /dpa /install.sh /directory /directory/* /directory.json /zetlyn /zetlyn/* /testwelt /testwelt/* /worlds/* /app/* /t/* /trackers /trackers/* /sources /sources/* /scopes /datasets /api /hub-about
	redir @old / 302
	# The old account's billing pages; /billing/stripe is where Stripe sends its events.
	@oldbilling {
		path /billing/*
		not path /billing/stripe
	}
	redir @oldbilling / 302

	reverse_proxy 127.0.0.1:4848

	handle_errors {
		respond "Zetlyn is not answering just now. Try again in a minute.
" {err.status_code}
	}

	header {
		X-Content-Type-Options nosniff
		Referrer-Policy strict-origin-when-cross-origin
		# Always https, for a year; and no page of Zetlyn's in another site's frame (the page to
		# mark frames Zetlyn's own /frame, which is the same site).
		Strict-Transport-Security "max-age=31536000"
		X-Frame-Options SAMEORIGIN
		-Server
	}
}

www.zetlyn.com {
	redir https://zetlyn.com{uri} permanent
}
