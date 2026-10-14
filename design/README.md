# design

Source for the beardictate design canvas (Claude Design `.dc.html` artboards):
logo explorations, tray icon states + menu, notifications + pin-the-tray,
modals, settings window, first run. `canvas.json` is the board layout.

## The mark

**Quote Bear** (name kept from the exploration, though the quote-mark ear
tails are gone): a navy bear head on a cobalt tile, pale inner ears, a soft
teddy nose, and a five-bar sound wave under it whose middle bar runs up to
the nose. Final files are in [`logo/`](logo/):

| File | Use |
|---|---|
| `mark.svg` | 48 px and up: full detail |
| `mark-small.svg` | 24–32 px: no sound bars |
| `glyph-16-light.svg`, `glyph-16-dark.svg` | 16 px: no tile, silhouette with a cobalt snout |
| `tray-32-light.svg`, `tray-32-dark.svg` | Tray glyph; the snout colour and corner dot carry state (see `canvas/Tray.dc.html`) |

| Token | Hex | Role |
|---|---|---|
| Cobalt | `#2F6FDB` | Tile, accent fills |
| Cobalt text | `#2459B8` | Links, accent text, primary buttons (white text) |
| Cobalt deep | `#173F86` | Hover, badge text |
| Navy | `#0E1A30` | The bear |
| Ice | `#DDE8FB` | Inner ears, eyes, muzzle |

UI neutrals stay warm (ink `#241C16`, greys `#6B5D50`/`#5E5146`, paper
`#F3EEE6`). UI type Figtree, prose samples Literata.

How we got here (cobalt over honey, Snug spacing, 112% scale, nose and inner
ears to stop it reading as a pig, the philtrum-as-bar mouth) is in the
comparison pages `quote-bear-*.html`.
