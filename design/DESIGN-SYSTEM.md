# Springen Design System

Springen is a map design tool for **Spring / Recoil RTS maps**. You author terrain as a
node graph of resolution-independent fields, watch the engine's own layer manifest update
as you work, place metal spots against real extractor-radius validation, and bake the full
SMF layer set — heightmap, diffuse, metal, type, grass, splat, `mapinfo.lua`,
`metal_layout.lua`.

The shipped product is a **desktop application for Windows and Linux, built in Rust with
wgpu**. The design system in this project is the visual language for that build.

## Sources

- `mapproj/mapforge.html` — the browser beta ("MapForge"), a single 3,734-line HTML file,
  attached read-only via the local-folder mount. It is the ground truth for information
  architecture, terminology, node inventory, engine math and copy tone. Every token, layout
  rule and component here traces back to it.
- No Figma file, no repository, no existing brand assets were provided.

## The rework, in one line

The beta looked like a game mod tool: condensed uppercase type, teal-tinted chrome, chunky
tracked buttons. Springen looks like a **workstation**: neutral cool graphite so the terrain
is the only saturated thing on screen, one accent, 22–32px control heights, hairline rules,
and mono for every number the engine produces.

---

## Content fundamentals

**Voice.** A competent colleague who knows the engine. Second person when instructing, no
first person, no marketing register, no exclamation. The beta's copy is the model — keep it.

**Casing.** Sentence case everywhere: buttons ("Propose from mask", "Open project…", "Bake
layers"), section titles in uppercase micro-labels only ("SPRING LAYER MANIFEST", "METAL
SPOTS"). Never title case. Never uppercase a button.

**Numbers over adjectives.** Springen states measured facts, not judgements.

> Heightmap 769 × 769 (16-bit)
> 8 spots proposed (target 14 — spacing too tight)
> Rejected: that link would create a cycle
> Two spots are 512 elmos apart; extractor radius is 90.

Not "Success!", not "Oops, something went wrong."

**Engine vocabulary is used precisely and never softened.** elmo, mapx/mapy, SMF, SMT, mex,
splat, talus angle, vertex lattice, 512-elmo unit, 16-elmo metal grid. When a constraint
comes from the engine, say so and say why: *"`mapx` must divide by 128, so the size unit has
to be even."*

**Hints teach the model, not the widget.** The beta's hint text explains the system's
reasoning — *"Every distance is authored in elmos, so changing working resolution never
changes the shape of the terrain."* Write hints like that or omit them.

**Errors name the cause and the value.** Never "invalid input". Always which node, which
spot, which number.

**No emoji. Ever.** Not in UI, not in docs, not in commit messages.

**Units are spelled with the value**: `3072 elmos`, `769 × 769`, `12.0 MB`, `32°`, `0.0076
elmo/LSB`. Multiplication uses `×`, squares use `²`, ellipsis in deferred actions uses `…`.

---

## Visual foundations

**Colour.** Chrome is a cool graphite ramp from `#070A0C` (app void) to `#3B444D` (strongest
border) — six surfaces, five ink weights, nothing else. There is exactly **one accent**:
contour orange `#E08A3C`, inherited from the beta and reserved for the active/selected thing,
the single primary action, and hero values in the manifest. Shoal cyan `#46B9C4` is the
secondary hue for engine-truth data: terminal nodes, derived values, links. Status red/amber/
green appear only on validation. The **hypsometric terrain ramp** (abyss → snow, lifted
verbatim from the beta evaluator) is the brand's colour imagery — saturated colour on a
Springen screen belongs to the terrain, never to the interface.

**Type.** Archivo (700/600) for the wordmark, marketing and dialog titles — engineering
signage, tight tracking. IBM Plex Sans for all UI: 13px body, 12px dense rows, 11px semibold
uppercase labels at 0.09em, 10px micro. IBM Plex Mono for every number, id, path and derived
value; mono values are always right-aligned. The beta's Barlow Condensed is retired — condensed
uppercase read as game UI and did not survive at workstation density.

**Spacing and density.** 4px grid with 2px and 6px half-steps, because tool chrome genuinely
needs them. Controls are 22 / 26 / 32px tall; table rows 20px; panel headers 32px. The window
is a fixed frame: 44px toolbar, 212px palette, fluid canvas, 344px inspector, 24px status bar.
Nothing in the chrome is centred; everything is flush to its rail.

**Backgrounds.** No photography, no illustration, no gradient decoration. Two textures exist:
the graph canvas grid (24px minor / 120px major hairlines on `--surface-canvas`) and the
terrain the tool itself generates. Panels are flat fills separated by 1px hairlines.

**Borders and shadow.** 1px hairlines carry almost all separation. Shadow is reserved for
things that genuinely float above the canvas — graph nodes (`0 6px 18px rgba(0,0,0,.45)`),
popovers, dialogs. Panels never have shadows. Inputs carry a subtle inset well instead.

**Corner radii.** 0 for panels, rows and rails; 2px for controls and nodes; 3px for popovers
and dialogs; pill only for the slider thumb and port dots. Nothing is rounder than 5px.

**Cards.** There are no cards. The one card-like object is the graph node: 184px wide, 2px
radius, 1px border, a 2px coloured top edge encoding its class (grey operator, cyan terminal,
olive texture), a 22px drag header, and its own evaluated 48² thumbnail.

**States.** Hover lightens the surface one step and, on list items, adds a 2px accent left
edge. Press darkens and nudges 0.5px — no scale, no bounce. Selection is accent border plus
`inset 2px 0 0` on rows. Focus is a 1px accent ring at 1px offset. Disabled is 38% opacity.

**Motion.** 80ms for hover/press, 120ms for control state, 180ms for panels and toasts, 260ms
for veils. `cubic-bezier(.22,.61,.36,1)`, ease-out only. No entrance animation, no springs, no
parallax. The only looping animation in the product is the indeterminate bake sweep, and it
stops under `prefers-reduced-motion`.

**Transparency and blur.** Blur is never used. Transparency appears in exactly three places:
the bake veil (78% scrim), status/accent tints behind validation text (12–22%), and unfilled
metal-spot markers on the preview.

**Imagery.** Cool, desaturated, hillshaded. Previews are shaded relief with optional contour
banding at sea-level flooding — the same painting the evaluator produces. Never stylise it.

---

## Iconography

The beta shipped **no icon set** — it used unicode `–` / `+` / `×` in steppers and text labels
everywhere else. That does not scale to the reworked toolbar, so Springen adopts **Lucide**
(14px in chrome, 16px in dialogs, 1.5px stroke) as a flagged substitution. **If Springen
commissions its own glyphs, swap the base URL in `components/icon/Icon.jsx` — that is the only
place icons are resolved.**

- The 31 glyphs in use are **vendored**: path data lives in `components/icon/Icon.jsx` and mirror
  files in `assets/icons/`. Nothing is fetched at runtime. `Icon.names` lists what is available;
  add a glyph in both places.
- Never inline hand-drawn SVG in a Springen surface. Never mix a second icon family.
- Never emoji. Unicode is used only for engine notation: `×`, `²`, `°`, `–`, `…`.
- Working set: `mountain-snow`, `waves`, `layers`, `git-branch`, `crosshair`, `ruler`,
  `grid-3x3`, `droplets`, `wind`, `download`, `play`, `settings-2`, `trash-2`, `frame`, `dices`,
  `zoom-in`, `zoom-out`, `cpu`, `check`, `x`, `triangle-alert`, `chevron-down`, `info`, `move`,
  `plus`, `minus`, `folder-open`, `save`, `undo-2`, `redo-2`, `eye`.

## Logo

`assets/logo.svg` (lockup), `assets/logomark.svg` (mark), plus `-dark` variants for light
backgrounds and `logomark-solid.svg` for favicons and window icons.

The mark is a **contour peak**: five nested contour lines from a real radial noise field,
clipped to a square tile so the outer rings run off the edge — a map fragment, not a badge.
The innermost ring is filled in contour orange: the summit, and the tool's accent. It reads
down to 16px. The wordmark is Archivo 700, uppercase, +0.5 tracking.

**This mark was created for this design system** — no logo existed in the provided sources.
Treat it as a proposal until the team signs off.

---

## Index

**Root**
- `styles.css` — the single entry point consumers link. `@import` list only.
- `tokens/` — `colors.css`, `typography.css`, `space.css`, `elevation.css`, `motion.css`,
  `fonts.css`, `base.css`.
- `components/springen-ui.css` — the class layer every component renders against.
- `assets/` — logo lockups and marks.
- `guidelines/` — 19 specimen cards (Colors, Type, Spacing, Brand).
- `ui_kits/springen-workstation/` — the product recreation. Three windows, in launch order:
  `splash.html` (preload) → `projects.html` (map browser) → `index.html` (workspace).
- `SKILL.md` — Agent Skills entry point.

**Components** (`window.SpringenDesignSystem_29c2a3.<Name>`)

| Group | Components |
| --- | --- |
| `components/chrome/` | `Toolbar`, `ToolbarGroup`, `Brand`, `Panel`, `PanelSection`, `StatusBar`, `StatusItem`, `TitleBar` |
| `components/controls/` | `Button`, `IconButton`, `Field`, `Select`, `Checkbox`, `Stepper`, `StepperDim`, `SliderRow`, `ControlRow`, `SegmentedTabs` |
| `components/data/` | `SpecTable`, `StatRow`, `DataTable`, `Badge` |
| `components/graph/` | `GraphCanvas`, `GraphNode`, `Wire`, `NodePalette` |
| `components/feedback/` | `Issue`, `Toast`, `ProgressVeil` |
| `components/project/` | `ProjectTile`, `SplashPanel` |
| `components/icon/` | `Icon` |

Each directory holds `<Name>.jsx`, `<Name>.d.ts`, `<Name>.prompt.md` and one card HTML.

### Intentional additions

The beta defines the inventory; these four have no direct counterpart in it and were added
for the desktop rework:

- **`StatusBar` / `StatusItem`** — the beta had nowhere to put persistent size, resolution and
  backend readouts; a desktop tool needs them out of the inspector.
- **`Icon`** — required by the substituted icon set (above).
- **`Badge`** — the beta encoded node class and validation state in ad-hoc coloured text.
- **`SegmentedTabs`** — a component extraction of the beta's `.pvtabs`, not a new pattern.
- **`TitleBar`, `ProjectTile`, `SplashPanel`** — the beta was a web page with one screen and no
  window chrome. The desktop build needs a preload window and a project browser before the
  workspace opens, so these three are **new design**, not recreations: they follow the system's
  own rules (mono facts, hairlines, one accent, relief thumbnails instead of icons) but have no
  counterpart in `mapforge.html`. Review them as proposals.

### Known gaps

- The beta's **texture graph** (Height ramp, Slope material, Bake lighting, Water tint,
  Splat distribution out) exists in the node palette but has no dedicated inspector UI in the
  kit.
- No dialog/modal component: the beta has none, and none was invented.
- Webfonts are pulled from Google Fonts, not vendored. The Rust build will need local
  `woff2`/`ttf` files and real `@font-face` rules in `tokens/fonts.css`.
