# The Folio mark

Folio's mark is a quire: three open sheets stacked one above another, the folded gathering from which a book is sewn. It was chosen on 2026-11-08 from four concepts because it says "book" without reaching for the open-book glyph that half of all reader apps use, and because it nods to the Quireware name.

## Geometry

We draw the mark on Android's 108-unit adaptive-icon grid, so the same paths serve the launcher, the app header and every brand file. Each sheet is one path with a 6.5-unit stroke and round caps and joins:

    M30 46C36 38 48 38 54 46C60 38 72 38 78 46
    M30 58C36 50 48 50 54 58C60 50 72 50 78 58
    M30 70C36 62 48 62 54 70C60 62 72 62 78 70

The paths, with half the stroke added, sit inside the grid's 66-unit safe circle, which leaves the clear space a launcher mask needs. `crates/folio-app/src/chrome.rs` holds these strings as `MARK_PATHS`, and its unit tests fail if the brand files or the launcher vector drift from them, or if a point leaves the safe circle.

## Colour

The mark is ink on paper or paper on ink: `#111111` and `#FFFFFF`. The launcher tile is ink with white sheets, which gives the highest contrast on the Boox's e-ink panel. The accent blue (`#2447C9`) is reserved for reading progress inside the app and is not used for the mark.

## Files

| File | Use |
|---|---|
| `assets/brand/folio-mark.svg` | The mark alone, in `currentColor`, for documents and inline use |
| `assets/brand/folio-icon.svg` | The ink tile, full bleed, the source for every raster |
| `assets/brand/folio-icon-512.png` | Repository and organisation avatar |
| `assets/android/res/` | Launcher overlay copied over `dx`'s icons by `scripts/build-apk-debug.sh` |

After changing the mark, update `MARK_PATHS` and the SVGs together, then run `scripts/render-icons.sh` to regenerate the rasters.

## Wordmark

In the app the wordmark is "Folio" set in the platform sans at weight 700 with −0.035em tracking, beside the mark. We do not ship it as an outlined file, because that would need a bundled font.
