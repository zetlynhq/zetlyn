# Running zetlyn.com

How the hosted service at zetlyn.com is built and run; the files beside this one are its units,
Caddy configuration, backup and restore. Nothing here is needed to run an instance of your own
(see the README at the top).

The server is n1 (`root@zetlyn.com`). The program is `/srv/zetlyn/zetlyn`, its data
`/var/lib/zetlyn` (one SQLite file), its settings `/etc/zetlyn/env`, `/etc/zetlyn/stripe.env` and
`/etc/zetlyn/smtp-password`; Caddy hands every address to `127.0.0.1:4848`.

```sh
sh deploy/build-linux.sh              # in Docker, on Ubuntu 24.04 → dist/amd64/zetlyn
scp dist/amd64/zetlyn root@zetlyn.com:/srv/zetlyn/zetlyn.new
ssh root@zetlyn.com 'cd /srv/zetlyn && cp zetlyn zetlyn.prev && mv zetlyn.new zetlyn && systemctl restart zetlyn && sleep 2 && curl -s 127.0.0.1:4848/health'
```

Back: `cp /srv/zetlyn/zetlyn.prev /srv/zetlyn/zetlyn && systemctl restart zetlyn`.

- `zetlyn.service`: the app. `zetlyn-alert.timer`: every 5 minutes, mail to alarm@zetlyn.com when
  something is wrong and again when it is right. `zetlyn-backup.timer`: daily 03:30 UTC, the data
  checked, compressed, encrypted with `/etc/zetlyn/backup.key` and put in the object storage
  (`deploy/backup.sh`); `deploy/restore.sh daily/Mon /tmp/x.db` brings one back into a file of its
  own. Keep a copy of `backup.key` away from the server.
- Caddy: write `deploy/Caddyfile.zetlyn.com` to `/etc/caddy/Caddyfile.candidate`, check it with
  `caddy validate`, keep the old file, then move it in place and `systemctl reload caddy`.
- Commands on the server run as the service's user, so the file stays its own:
  `systemd-run --wait --pipe --collect -p DynamicUser=yes -p User=zetlyn-app -p StateDirectory=zetlyn /srv/zetlyn/zetlyn tally --data /var/lib/zetlyn`


## Listing in directories

`listing/` has the texts and icons for MCP directories, `chatgpt-plugin/` the ChatGPT plugin.
