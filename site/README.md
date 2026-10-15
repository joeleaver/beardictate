# beardictate.jkbase.app

The public site for [beardictate](https://github.com/joeleaver/beardictate): a landing page, a
download page and a help page. It's a plain static jkbase site (`public/`). There's no build
step, it makes no third-party requests and sets no cookies.

## Where the downloads live

The installers are files in the site itself, under `public/downloads/`. The download page
renders `public/downloads/releases.json`, so a new build means a data change and a redeploy,
never a page edit.

This is enough because the site only hosts the **installers**. beardictate downloads its models
on first run from pinned, hashed upstream URLs (see `crates/bd-models/manifest.toml` in this
repo), so this site never serves the 0.6–2.5 GB weights. A Windows build is about 26 MB zipped.
The in-VM static server serves Range requests and ETags, so interrupted downloads resume.

Files aren't committed. `.gitignore` keeps everything in `public/downloads/` except the manifest
out of git, so **the checkout you publish from holds the only local copy of the binaries**. If
you deploy from a checkout that doesn't have them, `publish.sh deploy` refuses, and the page
can't end up with dead links.

## Publish a build

```bash
cd site
./publish.sh add windows 0.1.0 /path/to/beardictate-0.1.0-windows-x64-setup.exe
./publish.sh add windows 0.1.0 /path/to/beardictate-0.1.0-windows-x64.zip
./publish.sh check                  # every listed file present, size + SHA-256 match
./publish.sh deploy                 # check, then `jkbase deploy`
```

`add` copies the file into `public/downloads/`, writes `<file>.sha256` beside it (for
`sha256sum -c`), and records its size, hash and date. It also drops that platform's
placeholder entries. The label shown on the button comes from the extension (`-setup.exe` →
Installer, `.zip` → Portable zip, `.AppImage`, `.deb`, `.dmg`, …); override it with
`--kind "…"`. Platforms are the ids in `releases.json` (`windows`, `linux`, `macos`), and each
one's requirements line and "not yet" message live there too.

To trim old builds, run `./publish.sh prune windows 2` (keeps the newest 2 versions), and
`./publish.sh remove <file>` to delete one. Older versions on the page fold under "Earlier
versions".

Commit `releases.json` after each publish, so the repo records what's live.

## Placeholders

Until real builds exist, `releases.json` carries entries with `"placeholder": true`. The page
shows them with a disabled button and a "Placeholder: not published yet" tag. `add` removes
them for the platform it publishes.

## First deploy

```bash
jkbase project create beardictate   # → https://beardictate.jkbase.app
cd site && ./publish.sh deploy
```

Each download counts against the project's bandwidth quota. Raise it as an admin if the
installers get popular.
