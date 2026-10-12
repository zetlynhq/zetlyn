#!/bin/sh
# A world's heavy work in a Linux container of a given memory, as a cell of that size would do it:
# each step's result, and the most memory the container held by then, as the kernel counts it for
# a cell (its files in memory included). A step the kernel ends for its memory says so (137).
#
#   sh deploy/memtest.sh <workspace> [1g] [tracker]
#
# The workspace is changed: its tracker is rebuilt and its largest source published into a folder.
# Run it on a copy. The Linux binary is built first where dist/amd64 has none.
set -eu

[ $# -ge 1 ] || { echo "sh deploy/memtest.sh <workspace> [1g] [tracker]" >&2; exit 2; }
WS=$(cd "$1" && pwd)
MEM=${2:-1g}
TRACKER=${3:-cve}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOT/dist/amd64/zetlyn"
[ -x "$BIN" ] || sh "$ROOT/deploy/build-linux.sh"

# The largest source, which is the one a version for the hub is hardest to make of.
LARGEST=$(cd "$WS/sources" && ls -S */claims.db 2>/dev/null | head -1 | cut -d/ -f1)

docker run --rm --platform linux/amd64 --memory "$MEM" --memory-swap "$MEM" \
	-e SQLITE_TMPDIR=/var/tmp -e ZETLYN_HOME=/w/.memtest-home \
	-v "$WS":/w -v "$BIN":/usr/local/bin/zetlyn:ro -w /w \
	ubuntu:24.04 sh -c '
peak() { echo $(( $(cat /sys/fs/cgroup/memory.peak) / 1048576 )); }
failed=0
step() {
	name=$1; shift
	started=$(date +%s)
	if "$@" > /tmp/said 2>&1; then result=ok; else result="FAILED ($?)"; failed=1; fi
	echo "$name: $result in $(( $(date +%s) - started )) s, the container at most $(peak) MB of '"$MEM"'"
	tail -2 /tmp/said | sed "s/^/    /"
}
step "rebuild trackers/'"$TRACKER"'" zetlyn tracker refresh trackers/'"$TRACKER"' --rebuild
step "refresh trackers/'"$TRACKER"' whole" env ZETLYN_REFRESH_WHOLE=1 zetlyn tracker refresh trackers/'"$TRACKER"'
rm -rf /w/.memtest-hub && mkdir -p /w/.memtest-hub
step "publish sources/'"$LARGEST"'" zetlyn source publish sources/'"$LARGEST"' --to /w/.memtest-hub
step "publish sources/'"$LARGEST"' again (delta)" zetlyn source publish sources/'"$LARGEST"' --to /w/.memtest-hub
rm -rf /w/.memtest-hub /w/.memtest-home
exit $failed
'
