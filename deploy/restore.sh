#!/bin/sh
# A backup brought back, into a file of its own, never over the live data:
#
#   sh restore.sh daily/Mon /tmp/zetlyn-restored.db
#
# Then look at it (sqlite3), and only by hand, with the service stopped, put it in place.
set -eu
[ $# -eq 2 ] || { echo "usage: restore.sh daily/<Mon…Sun>|monthly/<yyyy-mm> <new file>" >&2; exit 2; }
[ ! -e "$2" ] || { echo "$2 is there already" >&2; exit 1; }
. /etc/zetlyn/s3.env
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl --fail --silent --show-error --aws-sigv4 "aws:amz:$ZETLYN_S3_REGION:s3" \
	--user "$ZETLYN_S3_KEY:$ZETLYN_S3_SECRET" -o "$TMP/b.enc" \
	"$ZETLYN_S3_ENDPOINT/$ZETLYN_S3_BUCKET/backups/zetlyn/$1.db.gz.enc"
openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -pass file:/etc/zetlyn/backup.key -in "$TMP/b.enc" | gunzip > "$2"
echo "$2: $(sqlite3 "$2" 'PRAGMA integrity_check'), $(sqlite3 "$2" 'SELECT COUNT(*) FROM accounts') accounts, $(sqlite3 "$2" 'SELECT COUNT(*) FROM watches') watches"
