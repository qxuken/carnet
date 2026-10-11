#!/bin/sh
# A server's container image, put together and pushed without Docker: what
# CI does, on a runner whose jobs have no Docker daemon to build with.
# packaging/server/Dockerfile stays the way to build one by hand; this
# makes the same image from a binary and the web viewer built beforehand
# (on the same Debian as the base, so the binary's glibc is the image's).
#
#   sh packaging/server/image.sh --binary target/release/carnet-server \
#       --web web/dist --env CARNET_DATA=/data --env CARNET_LISTEN=0.0.0.0:8080 \
#       --env CARNET_WEB=/usr/share/carnet/web REF [REF...]
#
# The first REF is built and pushed; the others are copies of it (another
# tag, another registry). Each registry's login is crane's
# (`crane auth login`). The image:
#
#   - debian:bookworm-slim, for $PLATFORM (linux/amd64 unless said)
#   - the binary at /usr/local/bin/<its name>, its entrypoint
#   - the viewer at /usr/share/carnet/web
#   - /data, owned by uid 10001, which it runs as; its working directory
#     and a volume
#   - port 8080, and a health check if --health is given (`<binary>
#     health`, as the Dockerfile's)
#
# CRANE names crane (go-containerregistry's), `crane` on PATH unless said.
set -eu

crane=${CRANE:-crane}
platform=${PLATFORM:-linux/amd64}
binary= web= health= envs=
while [ $# -gt 0 ]; do
    case $1 in
        --binary) binary=$2; shift 2 ;;
        --web) web=$2; shift 2 ;;
        --env) envs="$envs --env $2"; shift 2 ;;
        --health) health=1; shift ;;
        --) shift; break ;;
        -*) echo "image.sh: what is $1?" >&2; exit 2 ;;
        *) break ;;
    esac
done
[ -n "$binary" ] && [ -n "$web" ] && [ $# -gt 0 ] || { echo "image.sh: --binary, --web and a REF, at least" >&2; exit 2; }
name=$(basename "$binary")
first=$1
shift

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The layer: root's files root's, /data the server's, every time the
# same (no clock in it), so the same build is the same layer.
python3 - "$binary" "$web" "$work/layer.tar" "$name" <<'EOF'
import os, sys, tarfile
binary, web, out, name = sys.argv[1:]
def add(t, path, arcname, uid=0):
    info = t.gettarinfo(path, arcname)
    info.uid = info.gid = uid
    info.uname = info.gname = ""
    info.mtime = 0
    if info.isfile():
        with open(path, "rb") as f:
            t.addfile(info, f)
    else:
        t.addfile(info)
def dir(t, arcname, uid=0):
    info = tarfile.TarInfo(arcname)
    info.type, info.mode, info.uid, info.gid = tarfile.DIRTYPE, 0o755, uid, uid
    t.addfile(info)
with tarfile.open(out, "w", format=tarfile.PAX_FORMAT) as t:
    for d in ["usr", "usr/local", "usr/local/bin", "usr/share", "usr/share/carnet"]:
        dir(t, d)
    add(t, binary, f"usr/local/bin/{name}")
    for root, dirs, files in os.walk(web):
        dirs.sort()
        rel = os.path.relpath(root, web)
        base = "usr/share/carnet/web" if rel == "." else f"usr/share/carnet/web/{rel}"
        dir(t, base)
        for f in sorted(files):
            add(t, os.path.join(root, f), f"{base}/{f}")
    dir(t, "data", uid=10001)
EOF

"$crane" append --platform "$platform" -b debian:bookworm-slim -f "$work/layer.tar" -t "$first"
# shellcheck disable=SC2086 # each --env is its own word
"$crane" mutate "$first" -t "$first" \
    --user 10001:10001 --workdir /data --entrypoint "/usr/local/bin/$name" \
    --env HOME=/data $envs --exposed-ports 8080/tcp \
    --label "org.opencontainers.image.title=$name" \
    --label "org.opencontainers.image.revision=${GITHUB_SHA:-unknown}"

# The volume and the health check, which crane's mutate has no flags for:
# its config edited, as `crane edit config` reads it from stdin.
"$crane" config "$first" | python3 -c '
import json, sys
c = json.load(sys.stdin)
cfg = c.setdefault("config", {})
cfg["Volumes"] = {"/data": {}}
if sys.argv[1]:
    cfg["Healthcheck"] = {"Test": ["CMD", "/usr/local/bin/" + sys.argv[2], "health"],
                          "Interval": 30_000_000_000, "Timeout": 10_000_000_000, "StartPeriod": 10_000_000_000}
json.dump(c, sys.stdout)
' "$health" "$name" | "$crane" edit config "$first" >/dev/null

for ref in "$@"; do
    "$crane" copy "$first" "$ref"
done
echo "image.sh: $first${*:+ and $*}: $("$crane" digest "$first")"
