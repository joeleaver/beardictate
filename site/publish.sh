#!/usr/bin/env bash
# Publish beardictate builds on beardictate.jkbase.app.
#
# The download page renders public/downloads/releases.json, so a new build is a data
# change: copy the file in, record its size + SHA-256, deploy. Binaries are gitignored
# (this checkout is their only copy until deployed); the manifest is committed.
#
#   ./publish.sh add <platform> <version> <file> [--kind LABEL] [--date YYYY-MM-DD]
#                    [--channel alpha]
#       Copy <file> into public/downloads/, hash it, write <file>.sha256 next to it, sign
#       it, and record it in releases.json (replacing that platform's placeholders and
#       any entry with the same file name). <platform> is an id from releases.json:
#       windows, linux, macos. --kind defaults from the extension (Installer, Portable
#       zip, …). --channel NAME: only builds made with BD_UPDATE_CHANNEL=NAME update to
#       it, and the page hides it.
#       The app installs updates only from signed builds: the Ed25519 key is
#       ~/.config/beardictate-release/update-signing.pem (BD_SIGNING_KEY to change it);
#       its public half is in crates/bd-app/src/update/verify.rs. Back it up: without
#       it, installed copies can't update themselves.
#   ./publish.sh remove <file-name>
#       Drop a build from the manifest and delete its file.
#   ./publish.sh prune <platform> <keep>
#       Keep only the newest <keep> versions of <platform>; delete the rest.
#   ./publish.sh check
#       Every published build in the manifest is present and matches its size + hash.
#   ./publish.sh deploy [jkbase deploy args…]
#       check, then `jkbase deploy` this site. Refuses if check fails, so a deploy from a
#       checkout without the binaries can never publish dead download links.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
downloads="$here/public/downloads"
manifest="$downloads/releases.json"

die() { echo "publish.sh: $*" >&2; exit 1; }
[ -f "$manifest" ] || die "missing $manifest"
command -v python3 >/dev/null || die "python3 is required"

# All manifest edits go through one Python helper so the JSON stays well-formed and
# consistently formatted. Args: <op> [op args…]; paths come from the environment.
manifest_py() {
    MANIFEST="$manifest" DOWNLOADS="$downloads" python3 -I - "$@" <<'PY'
import hashlib, json, os, re, sys

manifest, downloads = os.environ["MANIFEST"], os.environ["DOWNLOADS"]
SAFE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")

def load():
    with open(manifest, encoding="utf-8") as f:
        return json.load(f)

def save(m):
    tmp = manifest + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(m, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.replace(tmp, manifest)

def vkey(v):
    return [int(p) if p.isdigit() else p for p in re.split(r"[.-]", v)]

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

op, args = sys.argv[1], sys.argv[2:]
m = load()

if op == "add":
    platform, version, name, kind, date, channel, signature = args
    if platform not in {p["id"] for p in m["platforms"]}:
        sys.exit(f"unknown platform {platform!r}; known: {', '.join(p['id'] for p in m['platforms'])}")
    if not SAFE.match(name):
        sys.exit(f"file name {name!r} must be letters, digits and . _ + - only")
    path = os.path.join(downloads, name)
    digest, size = sha256(path), os.path.getsize(path)
    with open(path + ".sha256", "w") as f:
        f.write(f"{digest}  {name}\n")
    m["builds"] = [b for b in m["builds"]
                   if b["file"] != name and not (b["platform"] == platform and b.get("placeholder"))]
    build = {"platform": platform, "version": version, "kind": kind, "file": name,
             "size": size, "sha256": digest, "date": date}
    if signature:
        build["signature"] = signature
    if channel != "stable":
        build["channel"] = channel
    m["builds"].append(build)
    m["builds"].sort(key=lambda b: (b["platform"], vkey(b["version"])), reverse=False)
    save(m)
    print(f"{name}\n  {size} bytes\n  sha256 {digest}")

elif op == "remove":
    (name,) = args
    before = len(m["builds"])
    m["builds"] = [b for b in m["builds"] if b["file"] != name]
    if len(m["builds"]) == before:
        sys.exit(f"{name} is not in the manifest")
    save(m)

elif op == "prune":
    platform, keep = args[0], int(args[1])
    real = [b for b in m["builds"] if b["platform"] == platform and not b.get("placeholder")]
    versions = sorted({b["version"] for b in real}, key=vkey, reverse=True)
    drop = {b["file"] for b in real if b["version"] in versions[keep:]}
    m["builds"] = [b for b in m["builds"] if b["file"] not in drop]
    save(m)
    print("\n".join(sorted(drop)))

elif op == "check":
    bad = 0
    for b in m["builds"]:
        if b.get("placeholder"):
            print(f"placeholder  {b['file']} (shown as not published yet)")
            continue
        path = os.path.join(downloads, b["file"])
        if not SAFE.match(b["file"]):
            print(f"BAD NAME     {b['file']}"); bad += 1
        elif not os.path.isfile(path):
            print(f"MISSING      {b['file']}"); bad += 1
        elif os.path.getsize(path) != b["size"] or sha256(path) != b["sha256"]:
            print(f"MISMATCH     {b['file']} (size or sha256 differs from the manifest)"); bad += 1
        else:
            print(f"ok           {b['file']}")
    listed = {b["file"] for b in m["builds"]} | {b["file"] + ".sha256" for b in m["builds"]}
    for f in sorted(os.listdir(downloads)):
        if f != "releases.json" and f not in listed:
            print(f"unlisted     {f} (will be deployed but isn't on the page)")
    sys.exit(1 if bad else 0)

else:
    sys.exit(f"unknown op {op}")
PY
}

default_kind() {
    case "$1" in
        *-setup.exe|*.msi) echo "Installer" ;;
        *.zip) echo "Portable zip" ;;
        *.AppImage) echo "AppImage" ;;
        *.deb) echo "Debian package" ;;
        *.rpm) echo "RPM package" ;;
        *.flatpak) echo "Flatpak" ;;
        *.dmg) echo "Disk image" ;;
        *.tar.gz|*.tar.xz|*.tgz) echo "Archive" ;;
        *) echo "Download" ;;
    esac
}

cmd="${1:-}"; shift || true
case "$cmd" in
    add)
        [ $# -ge 3 ] || die "usage: add <platform> <version> <file> [--kind LABEL] [--date YYYY-MM-DD]"
        platform="$1" version="$2" src="$3"; shift 3
        name="$(basename "$src")"
        kind="$(default_kind "$name")"
        date="$(date -u +%Y-%m-%d)"
        channel=stable
        while [ $# -gt 0 ]; do
            case "$1" in
                --kind) kind="${2:?--kind needs a value}"; shift 2 ;;
                --channel) channel="${2:?--channel needs a value}"; shift 2 ;;
                --date) date="${2:?--date needs a value}"; shift 2 ;;
                *) die "unknown option $1" ;;
            esac
        done
        [ -f "$src" ] || die "no such file: $src"
        [[ "$version" =~ ^[0-9]+(\.[0-9]+)*([-.][A-Za-z0-9.]+)?$ ]] || die "version '$version' should look like 0.1.0"
        dest="$downloads/$name"
        if [ "$(realpath "$src")" != "$(realpath -m "$dest")" ]; then
            cp --reflink=auto "$src" "$dest.part" && mv "$dest.part" "$dest"
        fi
        [[ "$channel" =~ ^[a-z]+$ ]] || die "channel '$channel' should be a word like alpha"
        key="${BD_SIGNING_KEY:-$HOME/.config/beardictate-release/update-signing.pem}"
        signature=""
        if [ -f "$key" ]; then
            signature="$(openssl pkeyutl -sign -inkey "$key" -rawin -in "$dest" | od -An -v -tx1 | tr -d ' \n')"
            [ ${#signature} -eq 128 ] || die "signing $name failed"
        else
            echo "publish.sh: no signing key at $key: installed apps won't update to $name" >&2
        fi
        manifest_py add "$platform" "$version" "$name" "$kind" "$date" "$channel" "$signature"
        echo "Recorded in releases.json. Run ./publish.sh deploy to put it live."
        ;;
    remove)
        [ $# -eq 1 ] || die "usage: remove <file-name>"
        manifest_py remove "$1"
        rm -f "$downloads/$1" "$downloads/$1.sha256"
        ;;
    prune)
        [ $# -eq 2 ] || die "usage: prune <platform> <keep>"
        manifest_py prune "$1" "$2" | while read -r f; do
            [ -n "$f" ] && rm -f "$downloads/$f" "$downloads/$f.sha256" && echo "removed $f"
        done
        ;;
    check)
        manifest_py check
        ;;
    deploy)
        manifest_py check || die "fix the files above before deploying"
        cd "$here" && jkbase deploy "$@"
        ;;
    *)
        sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'
        [ -z "$cmd" ] || exit 2
        ;;
esac
