# Where the photographic materials came from

Eleven surfaces, supplied by the project owner, downsampled here and embedded
in the binary by `crates/springen-core/src/material.rs`.

| Springen key | Source set | Library the file names identify |
|---|---|---|
| `asphalt`  | Asphalt001          | ambientCG |
| `lawn`     | Grass004            | ambientCG |
| `dust`     | Ground054           | ambientCG |
| `scrub`    | Ground056           | ambientCG |
| `steppe`   | grass_path_3        | Poly Haven |
| `silt`     | coast_sand_04       | Poly Haven |
| `peat`     | brown_mud_02        | Poly Haven |
| `track`    | aerial_wood_snips   | Poly Haven |
| `concrete` | brushed_concrete    | cgbookcase |
| `pavement` | granular_concrete   | cgbookcase |
| `clay`     | brown_sand_plaster  | cgbookcase |

All three libraries publish under CC0. That is recorded here rather than
asserted in code: the sets arrived as ZIPs without their licence files, and the
attribution above is inferred from the naming conventions each library uses.
**Verify before redistributing anything built with these.**

## What was kept, and what was thrown away

Each source set ships Color, Normal (GL and DX), Roughness, Displacement and
ambient occlusion at 4096². Only two of those survive here:

- **Color** → `<key>.color.png`, 512² RGB
- **Displacement** → `<key>.height.png`, 512² greyscale

The normal map is *derived* from the height rather than shipped, which is how
every drawn material in this tool already works — `material::render` takes the
derivative and wraps it at the edges, so the normal map tiles with the albedo
by construction instead of by trust. Roughness became a single per-material
gloss constant. Ambient occlusion was dropped: the bake already lays down its
own AO from the terrain, and a second one baked into the tile double-darkens
every crevice on the map.

## Why 512 and not 4096

A Spring detail tile is small. The reference map's is 512², and the tile is
repeated across the whole map, so a 4K source is sixty-four times more pixels
than the format can carry. 4K would inflate the binary to something near half a
gigabyte for detail nobody can see.

## Why the downsample filter matters

The reduction is a **box filter at an exact 8:1 ratio**, so each output texel
is the mean of one whole 8×8 block and no sample is ever read from outside the
image.

That is not an aesthetic choice. A wider kernel — Lanczos, bicubic — reads past
the edge of the image and has to invent what is there, and inventing what is
past the edge is precisely how a seamless texture stops being seamless. Spring
repeats a detail tile across the entire map, so a seam is not a line, it is a
grid.

Every one of these was measured before it was committed, and the measurement is
kept as a test: `photographs_tile_as_well_as_they_claim_to` compares the step
across the wrap against the typical step inside the image, per axis. All eleven
came in at about 1.0 — the seam is no rougher than ordinary interior detail.

Note that this is a *different* property from the one
`every_material_is_periodic_in_both_axes` checks. That test proves our sampling
repeats exactly, and it would pass just as happily on a photograph whose left
edge does not match its right. Both tests are needed.

## Reproducing the conversion

The source ZIPs are not in the repository. To regenerate from a fresh download:

1. Extract each set's Color (`_Color`, `_diff`, or `-diffuse`) and Displacement
   (`_Displacement`, `_disp`, or `-displacement`) image — the three libraries
   use three different naming conventions.
2. Resize both to 512² with a **box** filter.
3. Save colour as RGB PNG and displacement as 8-bit greyscale PNG, named
   `<key>.color.png` and `<key>.height.png`.
4. Run `cargo test --release -p springen-core material` — the tiling tests will
   catch a filter that was not box.
