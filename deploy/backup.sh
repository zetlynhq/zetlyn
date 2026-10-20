#!/bin/sh
# Zetlyn's data, copied off the machine once a day: a consistent copy of the SQLite file taken
# while the instance runs, checked, compressed, encrypted with /etc/zetlyn/backup.key, and put in
# the object storage under backups/zetlyn/: daily/<weekday> (seven, each overwritten a week later)
# and monthly/<yyyy-mm> (the last day of each month stays, for twelve months: the copy of the
# month a year back is deleted, as the privacy page says). Private: no ACL is given.
#
# /etc/zetlyn/s3.env: ZETLYN_S3_ENDPOINT, ZETLYN_S3_REGION, ZETLYN_S3_KEY, ZETLYN_S3_SECRET,
# ZETLYN_S3_BUCKET. The key must also be kept somewhere other than this machine.
set -eu
. /etc/zetlyn/s3.env
KEY=/etc/zetlyn/backup.key
DB=/var/lib/private/zetlyn/zetlyn.db
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

sqlite3 "$DB" ".backup '$TMP/zetlyn.db'"
[ "$(sqlite3 "$TMP/zetlyn.db" 'PRAGMA integrity_check')" = ok ] || { echo "the copy does not check" >&2; exit 1; }
gzip -9 "$TMP/zetlyn.db"
openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt -pass "file:$KEY" -in "$TMP/zetlyn.db.gz" -out "$TMP/zetlyn.db.gz.enc"

object() {
	echo "$ZETLYN_S3_ENDPOINT/$ZETLYN_S3_BUCKET/backups/zetlyn/$1.db.gz.enc"
}
put() {
	curl --fail --silent --show-error --aws-sigv4 "aws:amz:$ZETLYN_S3_REGION:s3" \
		--user "$ZETLYN_S3_KEY:$ZETLYN_S3_SECRET" -T "$TMP/zetlyn.db.gz.enc" "$(object "$1")"
}
gone() {
	curl --fail --silent --show-error --aws-sigv4 "aws:amz:$ZETLYN_S3_REGION:s3" \
		--user "$ZETLYN_S3_KEY:$ZETLYN_S3_SECRET" -X DELETE "$(object "$1")"
}
put "daily/$(date -u +%a)"
put "monthly/$(date -u +%Y-%m)"
# The month a year back; deleting what is not there is no error.
old="monthly/$(date -u -d "$(date -u +%Y-%m-15) -12 months" +%Y-%m)"
gone "$old"
echo "backed up $(stat -c %s "$TMP/zetlyn.db.gz.enc") bytes as daily/$(date -u +%a) and monthly/$(date -u +%Y-%m); $old deleted"
