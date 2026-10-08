#!/bin/sh
# Every admin page as it looks, light and dark, from tests/fixtures/control: the binary serves a
# copy, the operator signs in by the link it prints, headless Chrome takes each page. The pictures
# go to dist/screens/<version>/, to be looked at beside the release before.
#
#   cargo build && sh deploy/admin-screens.sh
set -eu

HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN="$HERE/target/debug/zetlyn"
CHROME=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$HERE/Cargo.toml" | head -1)
OUT="$HERE/dist/screens/$VERSION"
WORK=$(mktemp -d)
PORT=${PORT:-2598}
[ -x "$BIN" ] || { echo "no debug binary: cargo build" >&2; exit 1; }
[ -x "$CHROME" ] || { echo "no Chrome at $CHROME (CHROME=…)" >&2; exit 1; }

cp -R "$HERE/tests/fixtures/control" "$WORK/control"
printf 'title: Zetlyn\nurl: http://127.0.0.1:%s\naccess:\n  owners: [op@example.org]\n' "$PORT" > "$WORK/control/workspace.yaml"
"$BIN" hosting serve "$WORK/control" --addr "127.0.0.1:$PORT" --no-updates > "$WORK/log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null; wait $SERVER 2>/dev/null; rm -rf "$WORK"' EXIT
sleep 2

curl -s -o /dev/null -X POST -d "email=op@example.org" "http://127.0.0.1:$PORT/account/signin"
LINK=$(grep -o "http://127.0.0.1:$PORT/account/signin/[A-Za-z0-9]*" "$WORK/log" | tail -1)
curl -s -o /dev/null -c "$WORK/jar" -b "$WORK/jar" "$LINK"
curl -s "http://127.0.0.1:$PORT/account/style.css" > "$WORK/style.css"

mkdir -p "$OUT"
for page in "" new customers activity maintenance mail cell/acme cell/pilot; do
	name=$(printf '%s' "${page:-overview}" | tr '/' '-')
	curl -s -b "$WORK/jar" "http://127.0.0.1:$PORT/account/admin/$page" | sed "s#<link[^>]*stylesheet[^>]*>#<link rel=stylesheet href=\"file://$WORK/style.css\">#" > "$WORK/$name.html"
	sed 's#<html#<html data-theme="dark"#' "$WORK/$name.html" > "$WORK/$name-dark.html"
	for v in "$name" "$name-dark"; do
		"$CHROME" --headless=new --disable-gpu --hide-scrollbars --window-size=1360,1800 --screenshot="$OUT/$v.png" "file://$WORK/$v.html" >/dev/null 2>&1
	done
	echo "$OUT/$name.png"
done
