# Workspace UI visual design

The workspace UI (profile editor and run viewer, standalone and embedded) should feel like the same
product as [zeroshot.sh](https://zeroshot.sh). This guide is the visual source of truth for `ui/`.
Tokens live in [`src/styles.css`](src/styles.css); the theme preference lives in
[`src/theme.ts`](src/theme.ts).

## Direction

**Hand-marked paper.** Warm textured paper, black ink, hairlines and one hand-lettered wordmark,
like a well-kept engineering notebook. One channel is loud: the graph. Everything around it stays
quiet.

## Avoid

- Small tracked or ALL-CAPS labels, above headings or anywhere else (eyebrows, kickers). Labels are
  sentence case.
- A coloured or italic word inside a headline.
- Boxes inside boxes, or a large empty area with a centred icon.
- Coloured side borders thicker than 1px, tinted banners, glow, glass, gradient text.
- Status as boxed chips when a dot and a word will do.
- Native controls in system styling (blue checkboxes, OS select chevrons).
- Rust as the colour of an action.

## Color

Components use these semantic tokens, never raw values.

| Token                                                         |           Light (default) |                      Dark |
| ------------------------------------------------------------- | ------------------------: | ------------------------: |
| `--canvas` paper                                              | `#faf7f1` + paper texture | `#12110e` + chalk texture |
| `--panel` laid-on surface                                     |              white at 50% |             white at 3.5% |
| `--surface` solid surface (fields, menus, dialogs, inspector) |                 `#ffffff` |                 `#1b1915` |
| `--ink` primary text                                          |                 `#171411` |                 `#ede7db` |
| `--ink-secondary`                                             |                 `#4a433a` |                 `#c2b9aa` |
| `--ink-muted` (≥4.5:1 on the texture)                         |                 `#6a6053` |                 `#a19887` |
| `--hairline`                                                  |                 `#e4ded3` |                 `#312d27` |
| `--action` / `--on-action`                                    |               ink / paper |    ink / paper (inverted) |
| `--rust`                                                      |                 `#c2240c` |                 `#f06a4d` |
| `--ok-green` success                                          |                 `#387548` |                 `#6fc287` |
| `--amber` work in progress, watch                             |                 `#87580d` |                 `#d9a24a` |

The remaining tokens in `styles.css` (hover, grid, card border, edge, shadows) are editor surfaces
derived from these; add one there rather than a raw value in a component.

- **Ink is the action colour.** The primary button (Save profile, Save) is ink with paper text, and
  a screen has at most one. Secondary buttons are a hairline outline on the surface. Text buttons
  are the third tier. Replay controls are a tool to reach for, not the page's focus: play and the
  step buttons stay plain icon buttons. The focus ring, selection (a chosen node, a selected edge,
  the chosen option), checkboxes and the filled part of a range are ink too.
- **Rust marks state, not actions:** errors, invalid or missing values, failed runs and nodes, the
  current tab's underline, and remove controls on hover. Keep it small.
- **Green** marks success. **Amber** marks work in progress (running) and things to watch (an
  unsaved draft, a suspended run). A hollow `--ink-muted` ring marks work not yet started.
- Every state colour pairs with a word or an icon, and each reaches 4.5:1 on the paper.
- Light is the default. Dark applies when the user picks Dark, or picks System and the OS is dark.
  An embedding host sets the theme through the bridge.

## Type

- **Reenie Beanie** (the marker face) is the wordmark and top-level page titles only (such as
  Defaults), large (40–56px). Never for controls, headings inside a screen, or body text.
- **Spline Sans** (400–600) does everything else. A title about one item (a profile, a run) is 600
  at about 24px; inspector and dialog titles 600 at 18–20px.
- **System monospace** is functional: model and node identifiers, values, code, logs.
- Nothing below 12px; helper text 13px where there is room. Sentence case throughout.

## Wordmark

The crow and "zeroshot" in the marker face, with no rule or tag between them. The workspace kind
(Local, Target, Platform) follows as plain muted text, not a boxed tag. The embedded UI shows no
wordmark: the host owns that chrome.

## Composition and components

- Paper canvas with 1px hairlines. The header, toolbar and graph sit directly on the paper; the
  inspector, fields, menus and dialogs use the solid `--surface`.
- Fields are a white box with a hairline edge, an ink edge with focus, a faint ink tint when
  read-only, and a rust edge when the value is wrong. No tinted background for errors.
- Selects use the app's chevron (`--select-chevron`), not the operating system's.
- Tight radii (3–6px). Graph nodes are the exception: completion, junction and map shapes are
  rounder because the shape tells you what the node is.
- Status is a 6px dot and a word (`run-status`, `run-dot`). Success colours its word green and
  failure rust; running is an amber dot with a soft halo, suspended a plain amber dot, idle a hollow
  ring, anything else muted, and those words stay `--ink-secondary`.
- A notice is a hairline row on the panel or surface with a small dot leading the text: rust for an
  error, muted for a note. Never a tint or a thick edge, and no bold lead-in. It may carry one
  action on the right.
- Empty states are a short heading and one or two sentences. A spinner may lead while loading;
  otherwise no decorative icon.
- Shadows are rare and shallow; elevation comes from hairlines and surface contrast.

## Layout

Every page uses the same three layouts, and only these two widths (`max-width: 1150px` and
`max-width: 720px`):

- **Wide** (above 1150px): sidebar, canvas or list, and the inspector side by side.
- **Medium** (721–1150px): the sidebar goes behind its menu button; the inspector stays docked.
- **Narrow** (720px and below): the header takes two rows and the inspector slides over the canvas.

Moving to a narrower layout only folds things away; nothing hidden at one width comes back at a
narrower one. The top bar keeps one height (`--app-header-height`) and logo size at every width. Sidebars share
`--sidebar-width`, docked or popped out. Canvas toolbars keep one row
and answer to the canvas's own width (a container query), since the resizable inspector changes it
too: below 600px the editor's quieter tools fold into a **More tools** menu. Pages work down to a
360px-wide phone without clipping.

## Interaction

Motion communicates state: quick colour changes, about 150–250ms. Respect `prefers-reduced-motion`.
Keep strong keyboard focus, WCAG AA contrast, 44px touch targets on phones, and complete mobile
layouts.

When a screen-specific choice conflicts with this guide, preserve usability first and note the
exception beside the affected code.
