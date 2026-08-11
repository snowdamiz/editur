# Visual design plan

## 1. Purpose

Editur already looks like a real native editor: borderless chrome, a quiet dark palette, a custom retained-painted editor, and no widget-toolkit tells. It does not yet look like a *finished* one. The gap is not taste, it is consistency: the interface is assembled from per-call-site constants rather than from a design system, so surfaces, spacing, type sizes, stroke weights, corner radii, and semantic colors disagree with each other by small amounts in dozens of places. Small disagreements are exactly what the eye reads as "unpolished."

This plan defines the token layer Editur is missing, then applies it surface by surface, with each phase small enough to land and verify on its own.

### Audience and outcome

This plan is for the engineer doing the work. After reading it, they should be able to introduce the design tokens, migrate every existing call site, and rework the editor, tree, tabs, chrome, agent panel, and overlays without reopening visual decisions per pull request.

### Decision summary

- Replace the flat palette in `src/theme.rs` with a structured token module: elevation, state overlays, text roles, semantic colors, type scale, spacing, radii, control sizes, and motion.
- Express hover, press, and selection as alpha overlays instead of fixed colors, so the same interaction reads identically on every surface.
- Widen the elevation ramp so panels separate by luminance, and soften borders so hairlines stop being the loudest thing in the window.
- Reserve the cyan accent for "active right now." Everything decorative that is currently cyan becomes neutral.
- Derive the syntax palette from the same tokens as the chrome, and make the editor background exactly one value in one place.
- Add a real weight axis (Inter SemiBold as a second family) and a six-step type scale. Ship a modern default monospace instead of egui's bundled Hack.
- Fix the editor reading affordances that are currently missing or invisible: text selection, active line, active line number, wrapped-line indent, indent guides.
- Consolidate every hand-drawn glyph into one icon module on a 16px grid with one stroke weight.
- Rebuild the dialogs as one modal system with a fixed anatomy, a scrim, keyboard defaults, and a destructive variant, and move non-blocking errors out of a modal and into a toast.
- Introduce a small motion layer that is safe for the retained renderer: animated values are quantized and folded into `mark_retained` revisions.
- Add an Appearance section to settings (theme, density, editor font, line height). Keep the existing product decisions: no status bar, no extra window, no new rendering dependency.

## 2. What the current build gets right

Do not regress these while doing the work.

- Borderless window with custom chrome, native traffic lights placement, and a real app bundle.
- Retained painting with per-line and per-row revision keys (`src/editor_surface.rs:521`, `src/tree_surface.rs:106`), which is why resize is 0.26 ms per frame.
- Overlay scrollbars that fade after activity (`src/scrollbar.rs:48-64`) instead of permanently occupying a gutter.
- A restrained, nearly monochrome base palette. The direction is right; only the intervals are wrong.
- Vector chrome instead of bitmap icons, so everything is resolution independent.
- An agent panel that is a first-class surface rather than a bolted-on webview.

## 3. Current-state findings

Every value below was read from the source and confirmed against a running build.

### 3.1 Surfaces read as one flat sheet, then are separated by a loud hairline

The palette steps are `CANVAS 20,20,22` → `SURFACE 24,24,26` → `SURFACE_RAISED 28,28,31` → `SURFACE_INPUT 31,31,34` → `SURFACE_HOVER 35,35,39` → `SURFACE_SELECTED 39,39,43` (`src/theme.rs:8-13`). Every step is three or four values out of 255, which is at or below the just-noticeable difference at this luminance on most panels. Rendered, the tree is `(20,20,22)`, the editor is `(24,24,26)`, and the agent panel is `(23,23,25)` — the window reads as one sheet.

Then `BORDER_SUBTLE 48,48,52` and `BORDER_STRONG 56,56,60` jump twenty-four to thirty-two values in a single pixel. The result is the opposite of what modern editors do: the regions themselves do not separate, so the hairlines have to carry all the separation, and being the highest-contrast edge in the frame, they read as drawn-on lines rather than as structure.

Compounding this, the titlebar and tab strip are painted `SURFACE`, the same value as the editor body (`src/app.rs:6843`, `src/editor_surface.rs:18`). The boundary between chrome and document exists only as the active tab's 2 px accent underline.

### 3.2 The accent is spent on decoration

`ACCENT 94,210,224` is used for the caret (`src/theme.rs:63`), selection stroke (`src/theme.rs:52`), active widget borders (`src/theme.rs:90`), the active tab underline (`src/app.rs:4500`), checkmarks (`src/components.rs:210`), primary buttons (`src/components.rs:45`) — and for the folder glyph on every directory row in the tree (`src/tree_surface.rs:156`). A project with ten top-level directories stacks ten saturated cyan glyphs down the sidebar, where they dominate the frame and compete with the one thing that is genuinely active — the current tab.

An accent color is a pointer. When it appears on every row of a list, it points at nothing.

### 3.3 Chrome and code use two unrelated color systems

The syntax theme is a hand-built One Dark derivative: foreground `210,215,225`, keyword `198,120,221`, string `152,195,121`, type `97,175,239`, escape `86,182,194` (`src/syntax.rs:86-124`). The chrome is neutral gray with a cyan accent. The two have different white points (`210,215,225` versus `TEXT_PRIMARY 230,230,234`) and different color temperatures, so code sits in the window like a pasted screenshot.

The syntax theme also declares `background: color(30,33,39)` (`src/syntax.rs:146`) while the editor actually paints `EDITOR_BACKGROUND = SURFACE = 24,24,26`. Nothing currently reads the theme background, but two sources of truth for one value is a bug waiting for the first feature that needs it (selection highlights, minimap, diff view).

### 3.4 Typography has no scale, and no weight axis at all

Sizes in use: 10, 12, 12.5, 13, 13.5, 14, 15, 16, 20, and 25. The tree renders directories at 13.5 and files at 13.0 in the same vertical list (`src/tree_surface.rs:203`), which makes the column visibly ragged. The agent panel overrides `TextStyle::Body` to 15.0 (`src/app.rs:6788`), so chat text is two points larger than every other piece of UI text in the window. Tabs are 12.0 (`src/app.rs:4514`).

More importantly, **the UI has no bold anywhere**. Only `InterVariable.ttf` is bundled (`assets/fonts/`), egui does not apply variable font axes, so it renders the default instance. `RichText::strong()` in egui only swaps in `visuals.strong_text_color()` — it does not change weight — and in several call sites an explicit `.color()` is applied alongside it, so the call does nothing at all (`src/components.rs:41`). Every hierarchy decision in the product is currently made with size and gray level alone, which is why headings look like body text that happens to be bigger.

The code font is egui's bundled Hack, asserted by test (`src/theme.rs:148-161`). It is legible and dated.

### 3.5 The editor is missing standard reading affordances

- **Text selection is nearly invisible.** `visuals.selection.bg_fill = SURFACE_SELECTED` (`src/theme.rs:51`) is `39,39,43` against an editor background of `24,24,26`. That is a contrast ratio of roughly 1.1:1. Selecting a paragraph barely changes the screen.
- **The active line is invisible.** `Color32::from_white_alpha(6)` (`src/editor_surface.rs:534`) is 2.4% white, which does not survive on screen.
- **The active line number is not emphasized.** All numbers render `TEXT_DISABLED` (`src/editor_surface.rs:467`), so the gutter gives no positional feedback.
- **Wrapped lines are indistinguishable from new lines.** A soft-wrapped continuation starts at the same x as a fresh line, so wrapped prose and wrapped code both misread as separate statements.
- **There are no indent guides**, which is the single most useful structural cue in a code editor.
- **A full-height 1 px `BORDER_STRONG` rule divides the gutter from the text** (`src/editor_surface.rs:497-503`). No current editor draws this; it splits the reading surface in half.
- **Line height is 18 px at 14 px type** (`src/editor_surface.rs:14`), a ratio of 1.29. VS Code computes about 1.35 and Zed defaults higher. The text sits tighter than the reference products it will be compared to.

### 3.6 The tree is decorative where it should be informational

Row height 26 with two different label sizes and two different label colors; indent guides drawn at `left + 13 + depth*16` and the disclosure chevron drawn at exactly the same x (`src/tree_surface.rs:114` and `:117-120`), so from depth 1 down the chevron sits on top of the guide it should replace. Labels are laid out with `layout_no_wrap` and cached by revision, then painted with the color baked in (`src/tree_surface.rs:199-216`), which means hovering or selecting a row changes the row fill but never the label color — the opposite of `components::selectable_row`, which does change it (`src/components.rs:92-98`). Long names hard-clip at the panel edge with no ellipsis and no tooltip. There is no root header, so **nothing in the window tells you which project is open**. There is no file-type or version-control differentiation, and `git` appears nowhere in the UI layer.

### 3.7 Tabs and chrome are sized by fiat

`TAB_WIDTH` is a fixed 176 px with a center-aligned, truncated label (`src/app.rs:4454`, `:4507-4527`). Center alignment means the first characters of a filename move as sibling tabs open and close, which is why every shipping editor left-aligns tab labels. Each tab also paints its own right-hand divider (`src/app.rs:4491-4495`) in addition to the active-tab underline, so the strip carries two competing separator systems.

Chrome row heights are 34 (titlebar), 30 (pane tab), 30 (terminal header), and 38 (find bar) — four values for the same visual role (`src/app.rs:88-123`, `src/terminal.rs:22-26`). The macOS traffic lights are hand-painted Unicode glyphs (`src/app.rs:1887-1902`), which will not match the system's hover glyphs or its unfocused-window gray.

### 3.8 Icons use five stroke widths for one visual family

Stroke widths currently in use for the same visual family: 1.0 (tree file outline), 1.2 (tree chevron), 1.4 (checkmark, settings back arrow), 1.5 (component chevron and close), 2.0 (tab underline). Chevrons are drawn as two independent `line_segment` calls (`src/components.rs:166-167`, `src/tree_surface.rs:124-152`), so the two strokes overlap at the tip instead of joining, producing a thickened notch at every disclosure arrow. Sizes and optical centers are decided per call site.

### 3.9 Overlay chrome uses two shadow recipes and eight corner radii

Dialogs use offset `[0,10]`, blur 32, spread 2, black alpha 170, with a `from_white_alpha(24)` outline (`src/components.rs:11-19`). The search palette uses offset `[0,8]`, blur 28, spread 2, black alpha 150, with a `rgba(255,255,255,22)` outline (`src/app.rs:9041-9054`). Radii in use: 3, 4, 5, 6, 7, 8, 10, 12 — with 4 and 5 both used for list-row fills in the same visual family (`src/tree_surface.rs:112` versus `src/components.rs:90`).

### 3.10 The dialogs are six one-offs wearing the same frame

`src/components.rs` provides a frame, a title helper, a button, and an action row, and every dialog then improvises on top of them. The shared parts are too thin to enforce anything, so no two dialogs behave the same way.

- **Every dialog names itself twice.** `dialog_window(ctx, "Unsaved changes", "unsaved_dialog")` passes a title into `egui::Window::new`, then `begin_dialog(ui, "Unsaved changes")` paints the title again inside the body. The window's own title bar is disabled (`src/components.rs:30`), so the first string never renders at all. Both strings are duplicated at all seven call sites (`src/app.rs:11541`, `:11560`, `:11587`, `:11624`, `:5651`, `:5756`, `:6847`) and nothing keeps them in sync.
- **Body text has four different treatments.** "Unsaved changes" colors its body `TEXT_MUTED` (`src/app.rs:11562`), "File changed on disk" and "Save before running Agent" use a bare `ui.label` that resolves to `TEXT_SECONDARY`, "Save As" uses a bare label as a field caption, and the error dialog uses `colored_label` with a hardcoded `255,125,125` that appears nowhere else in the product (`src/app.rs:6851`).
- **Spacing is per-dialog.** Only "Unsaved changes" adds `ui.add_space(18.0)` before its buttons (`src/app.rs:11563`). In every other dialog the action row sits directly against the body, so the same component has a different rhythm each time it opens.
- **The destructive action is hand-built.** "Discard" is a bespoke `egui::Button` with hardcoded `224,156,160` text on a `40,34,37` fill and a 78 px minimum width (`src/app.rs:11568-11574`), sitting in a row whose other buttons are `dialog_button` at 72 px. It is the only destructive action in the product and it is not a component.
- **Button order is accidental.** `dialog_actions` lays out right-to-left, so whichever button is added first ends up rightmost. That yields `[Cancel][Save and Run]`, `[Cancel][Discard][Save]`, and `[Cancel][Save As…][Reload]` — three different orders, with the destructive action parked between two safe ones.
- **No dialog handles the keyboard.** Nothing binds Enter to the primary action or Esc to cancel, nothing requests focus, and the "Save As" path field is not focused when it opens (`src/app.rs:11624-11637`), so the first thing a user does in that dialog is reach for the mouse.
- **Nothing behind the dialog dims.** There is no scrim, so a modal does not read as modal; the editor stays at full contrast a few pixels away and the dialog looks like a floating card that happens to be centered.
- **The error surface contradicts itself.** `draw_error` is a modal window with a Dismiss button, given the id `error_banner` and anchored to the top center (`src/app.rs:6847-6848`). It is neither a toast — it never auto-dismisses — nor an alert that asks a question. Every failure in the product, trivial or serious, interrupts the same way.
- **Width is fixed at 340 px regardless of content** (`src/components.rs:39`), so a long error message or a long destination path wraps inside a narrow box with no path-aware truncation.
- **The agent file picker borrows dialog chrome for a list** (`dialog_frame(ctx).inner_margin(0)`, `src/app.rs:11139`), which makes a third overlay species alongside the dialog and the search palette.
- **The debt is actively spreading.** The two newest dialogs, "New keybinding profile" and "Record shortcut" (`src/app.rs:5651`, `:5756`), reproduce the duplicated title, the ad hoc spacing, and the manual button wiring, because copying the existing pattern is currently the only way to make a dialog.

### 3.11 Agent panel density and markdown fidelity

The user's own message is rendered inside a bordered box that reads as a text input rather than as a sent message. Tool activity rows are full-width bordered cards with generous vertical padding, so a five-step turn fills the panel. Markdown list items append `"• "` directly into a single `LayoutJob` (`src/markdown.rs:69-72`), so wrapped bullet text returns to the bullet column instead of hanging under the first character. The composer is a 108 px well whose only send affordance is a hairline arrow glyph.

### 3.12 There is no motion

`style.animation_time = 0.0` (`src/theme.rs:47`) and `.fade_in(false)` on every dialog (`src/components.rs:31`). Hover fills, dialogs, panel toggles, and tab switches are all hard cuts. The only things that move are the scrollbar fade, the caret blink, the agent transcript's edge gradient, the hover marquee on long tool paths, and the disclosure chevron — all of them hand-rolled, each with its own timing. Hard cuts are a defensible performance choice given the retained renderer, and they are also why the interface feels mechanical next to Zed or VS Code.

### 3.13 There are no appearance settings

`Settings` contains only `languageServers` (`src/settings.rs:16-19`). There is no theme choice, no density choice, no editor font or size, and no light theme.

## 4. Design direction

Five principles, in priority order. When two conflict, the earlier one wins.

1. **Structure comes from surfaces, not lines.** Panels separate because their fills differ; hairlines are a whisper, not the wall.
2. **One accent, one meaning.** Cyan marks what is active or what the next action is. At most one accent element per region at a time.
3. **One physical rhythm.** A 4 px spacing grid, four radii, one stroke weight, one type scale, four control heights. Everything snaps to them.
4. **The document is the subject.** Chrome recedes: quiet fills, muted glyphs, no decoration that competes with code.
5. **Motion is feedback, not decoration.** 90 ms on hover and press, 140 ms on panels, nothing longer than 220 ms, and nothing that costs a frame when idle.

Explicitly preserved: the borderless custom chrome, the retained renderer and its revision discipline, the cyan brand accent, Inter for UI, the single-purpose "quick focused edits" product shape, and the existing decision that **there is no status bar** (`KEYBINDINGS_PLAN.md:34`, enforced by `editor_column_fills_the_window_without_a_statusbar` at `src/app.rs:14683`). Status-like information belongs in the pane header, which this plan strengthens rather than replaces.

## 5. The token layer

### 5.1 Module shape

Split `src/theme.rs` into a `theme` module directory so tokens are addressable and testable:

```
src/theme/mod.rs         // apply(), apply_to(), Theme struct, active-theme access
src/theme/color.rs       // palettes: surfaces, text, accent, semantic, syntax
src/theme/state.rs       // hover/press/selected/focus overlays, compositing helpers
src/theme/typography.rs  // font families, the type scale, editor text metrics
src/theme/metrics.rs     // space, radius, stroke, control sizes, chrome row heights
src/theme/motion.rs      // durations, easing, retained-safe animate()
```

Every constant is `pub(crate)` and every consumer imports from here. No module outside `src/theme/` may construct a color.

### 5.2 Surfaces (dark theme)

| Token | RGB | Hex | Applies to |
| --- | --- | --- | --- |
| `surface.sunken` | 16, 16, 18 | `#101012` | Window backdrop, split gutters, empty pane well |
| `surface.chrome` | 22, 22, 25 | `#161619` | Titlebar, tab strip, file tree, agent panel, pane and terminal headers |
| `surface.editor` | 30, 30, 34 | `#1E1E22` | Editor body, terminal body, markdown preview |
| `surface.raised` | 38, 38, 43 | `#26262B` | Menus, dialogs, palette, settings cards, agent tool rows |
| `surface.input` | 26, 26, 30 | `#1A1A1E` | Text fields and wells inside raised surfaces |

Steps of 6 and 8 replace steps of 3 and 4, and the chrome no longer shares a value with the document. `EDITOR_BACKGROUND` and the syntect theme background become the same token.

### 5.3 States, borders, and text

States are alpha overlays composited onto whatever surface is beneath, so a hovered tree row and a hovered menu row look like the same interaction. Today a hover in the tree is +15 and a hover in a dialog is +7, because `SURFACE_HOVER` is an absolute color.

| Token | Value | Use |
| --- | --- | --- |
| `state.hover` | white 6% | Pointer over any row, tab, or icon button |
| `state.press` | white 10% | Active press |
| `state.selected` | white 12% | Selected row in an unfocused list |
| `state.selected.focus` | accent 16% | Selected row in the focused list |
| `border.hairline` | white 8% | Chrome-to-content dividers, tab separators |
| `border.strong` | white 14% | Dialog, menu, and palette outlines |
| `border.focus` | accent 55%, 2 px inset | Keyboard focus ring, focused pane |

| Text token | RGB | Contrast on `surface.editor` | Use |
| --- | --- | ---: | --- |
| `text.primary` | 233, 234, 238 | 13.8:1 | Code, active tab, dialog titles, selected rows |
| `text.secondary` | 166, 168, 176 | 7.0:1 | Default UI text, inactive tabs, file names |
| `text.muted` | 136, 138, 146 | 4.8:1 | Line numbers, hints, metadata, placeholders |
| `text.on-accent` | 8, 26, 30 | — | Text on a solid accent fill |

Three grays replace the current four (`TEXT_PRIMARY`, `TEXT_SECONDARY`, `TEXT_MUTED`, `TEXT_DISABLED` all live within 104 values and are assigned inconsistently). Disabled state is `text.muted` at 55% alpha, not a fourth gray.

| Semantic | RGB | Replaces |
| --- | --- | --- |
| `danger` | 232, 106, 102 | `235,91,91` and `238,132,139` (`src/app.rs:12515`, `:509`) |
| `warning` | 224, 176, 84 | `224,174,76` and `245,184,77` (`src/app.rs:12516`, `:4533`) |
| `success` | 118, 199, 150 | `105,210,157` (`src/app.rs:507`) |
| `info` | 108, 158, 214 | `104,155,207` (`src/app.rs:12518`) |

Each semantic also gets a `.subtle` form at 16% for diff and diagnostic backgrounds. `PANE_FOCUS_BORDER 75,101,128` (`src/app.rs:91`) is deleted in favor of `border.focus`.

**Editor-specific tokens:**

| Token | Value | Note |
| --- | --- | --- |
| `editor.selection` | accent 22% → composites to ≈ `44,70,76` | 8.4:1 against `text.primary`; replaces the 1.1:1 selection |
| `editor.selection.inactive` | accent 11% | Unfocused pane |
| `editor.line.active` | white 5% | Replaces the invisible 2.4% |
| `editor.indent.guide` | white 6% | New |
| `editor.indent.guide.active` | accent 30% | Guide for the enclosing block |
| `editor.wrap.indent` | 2 spaces + a 6 px `text.muted` tick | New |

### 5.4 Typography

Bundle a static **Inter SemiBold** instance as a second family (`FontFamily::Name("ui-strong")`) so weight becomes a real axis. Replace Hack with **JetBrains Mono** (OFL, subsettable) as the default code font, user-overridable.

| Role | Size / line height / family | Applies to |
| --- | --- | --- |
| `ui.micro` | 11 / 16 / semibold, +6% tracking, uppercase | Section labels ("EDITOR", "YOU"), badges |
| `ui.small` | 12 / 18 / regular | Tab labels, tree rows, metadata, hints |
| `ui.body` | 13 / 20 / regular | Default UI text, menus, buttons, agent transcript |
| `ui.strong` | 13 / 20 / semibold | Active tab, selected row, field labels |
| `ui.title` | 15 / 22 / semibold | Dialog and section titles |
| `ui.display` | 20 / 28 / semibold | Settings page titles, empty state headline |

The tree stops using two sizes; directories get `ui.small` in `ui.strong` weight, files get `ui.small` regular. The agent panel drops its 15 px override to `ui.body`.

Editor text: 14 px default (unchanged), line height **20 px (1.43)** replacing 18, with a `lineHeight` setting mapping compact 1.30 / standard 1.43 / comfortable 1.60. Font size range 10–24.

### 5.5 Space, radius, stroke, control sizes

- **Space:** 2, 4, 6, 8, 12, 16, 20, 24, 32. Every `add_space`, margin, and inset uses one of these.
- **Radius:** 4 (rows, chips, tags), 6 (buttons, inputs, icon-button hover), 10 (cards, menus, popovers), 14 (dialogs and the palette). The window's own radius is a platform value rather than a scale step: `WINDOW_CORNER_RADIUS` moves 10 → 12 to sit correctly inside the macOS window mask.
- **Stroke:** 1.0 for dividers, **1.25 for all icons**, 1.5 for the caret, 2.0 for the focus ring and the active-tab marker.
- **Control heights:** 24 (compact icon button), 28 (list row, tree row), 32 (button, input, tab), 36 (primary action, composer send).
- **Chrome rows:** 34 for the titlebar strip (retains the macOS traffic-light fit), 30 for secondary headers (pane header, terminal header), 34 for the find bar (down from 38). Three values, each with a stated reason.
- **Shadows:** exactly two. `shadow.popover` = offset `[0,6]`, blur 20, spread 0, black 45%. `shadow.dialog` = offset `[0,16]`, blur 40, spread 0, black 55%. Both call sites in `src/components.rs:14` and `src/app.rs:9049` adopt them.

### 5.6 Motion

| Token | Duration | Easing | Use |
| --- | ---: | --- | --- |
| `motion.fast` | 90 ms | cubic-out | Hover and press fills, icon color, tab activation |
| `motion.base` | 140 ms | cubic-out | Dialog and palette fade + 0.98→1.0 scale, menu open |
| `motion.slow` | 200 ms | cubic-in-out | Sidebar and agent panel width, agentic-mode transition |

Implemented as `theme::motion::animate(ctx, id, target, duration) -> f32` wrapping `Context::animate_bool_with_time_and_easing`, with two rules the retained renderer requires:

1. The returned value is **quantized to 1/64** before use, so an in-flight animation produces a bounded number of distinct frames rather than a new float every frame.
2. The quantized value is **folded into the `mark_retained` revision** of every shape it affects, following the pattern already used for scrollbar opacity (`src/scrollbar.rs:97-113`). A surface that animates without folding will paint a stale cached frame.

`style.animation_time` stays 0.0; nothing relies on egui's implicit widget animation. Motion is opt-out through an Appearance setting for users who want the current instant behavior.

## 6. Implementation phases

Each phase is independently shippable and independently revertible. Phases 2, 3, and 4 carry most of the perceived improvement; the phases before them exist to make those three safe.

### Phase 0 — Token module, no visual change

**Goal:** land the module structure and the new constants alongside the old ones, with the old names as deprecated aliases pointing at the closest new value. Nothing on screen moves.

**Changes:** create `src/theme/{mod,color,state,type,metrics,motion}.rs`; keep `apply()`/`apply_to()` behavior identical; add `contrast_ratio()` and `composite()` helpers.

**Tests:** contrast ratios for every text token against every surface it is allowed on; elevation ramp is strictly increasing with a minimum step of 6; `composite(state.hover, surface)` is monotonic.

**Done:** `cargo test` green, screenshots byte-identical to `main`.

### Phase 1 — Migrate every hardcoded color

**Goal:** remove the 100 literal color constructors that live outside the theme module — 70 in `src/app.rs`, 20 in `src/terminal.rs`, 3 each in `src/components.rs` and `src/scrollbar.rs`, 2 each in `src/editor_surface.rs` and `src/syntax.rs` — along with the 13 raw `color(r, g, b)` scope values in `src/syntax.rs` and the 20 bare `Color32::WHITE`-style fallbacks.

**Changes:** diagnostics, agent status pills, diff colors, drag ghosts, dirty-tab dot, and traffic-light fills map onto semantic tokens. The terminal's 16-color ANSI table becomes `theme::color::ansi()`, tuned to the same white point as the chrome. `src/syntax.rs:86-124` builds its scopes from `theme::color::syntax`, and its background becomes `surface.editor` exactly.

**Tests:** a source-scan test using `include_str!` over `app.rs`, `terminal.rs`, `components.rs`, `scrollbar.rs`, `editor_surface.rs`, and `tree_surface.rs` asserting no `Color32::from_*(` constructor appears outside `src/theme/`. This is one cheap test that permanently prevents the drift this plan exists to fix.

**Done:** the scan test passes; visual diff limited to the small hue corrections in the semantic set.

### Phase 2 — Surfaces, borders, and window structure

**Goal:** make the window read as three planes instead of one.

**Changes:** apply the new ramp; repaint the titlebar and tab strip as `surface.chrome` while the editor becomes `surface.editor`; convert all dividers to `border.hairline`; convert every hover and selection fill in `src/components.rs`, `src/tree_surface.rs`, and `src/app.rs` to the composited state overlays; standardize the four radii and the two shadows; `WINDOW_CORNER_RADIUS` 10 → 12; delete `PANE_FOCUS_BORDER` for `border.focus`.

**Tests:** the chrome fill and the editor fill differ by at least 6 per channel; a hovered tree row and a hovered dialog row produce the same delta from their respective backgrounds.

**Done:** at a glance you can tell where the sidebar ends and the document begins without looking for the hairline.

### Phase 3 — Typography

**Goal:** give the interface a weight axis and one scale.

**Changes:** bundle Inter SemiBold and register `FontFamily::Name("ui-strong")`; bundle JetBrains Mono and make it the default `Monospace` family; replace every literal `FontId::proportional(n)` with a scale role; delete the agent panel's `TextStyle` overrides (`src/app.rs:6786-6794`); unify the tree's two sizes; audit every `.strong()` call and either give it the semibold family or remove it.

**Tests:** update `font_definitions_use_inter_for_ui_without_replacing_code_font` (`src/theme.rs:147`) for the new families; assert `ui.strong` resolves to a different family than `ui.body`; assert the scale has no duplicate sizes.

**Done:** binary growth stays well inside the 30 MiB budget (expect roughly +0.6 MiB for two subset faces); record the new size in `PERFORMANCE.md`.

### Phase 4 — The editor surface

**Goal:** the largest single perceived improvement, and the one users feel while typing.

**Changes to `src/editor_surface.rs`:**

| Item | Now | Target |
| --- | --- | --- |
| Selection fill | `SURFACE_SELECTED`, 1.1:1 | `editor.selection`, plus a dimmed form for unfocused panes |
| Active line | `from_white_alpha(6)` | `editor.line.active`, and skipped entirely while a selection exists |
| Active line number | `TEXT_DISABLED` like all others | `text.primary`; others `text.muted` |
| Gutter rule | Full-height `BORDER_STRONG` line (`:497-503`) | Removed; replaced by a scroll shadow that appears only when scrolled horizontally |
| Wrapped lines | Continuation at column 0 | Continuation indented to the line's own indent + 2, with a `text.muted` tick in the gutter |
| Indent guides | None | 1 px `editor.indent.guide` every tab stop, enclosing block at `editor.indent.guide.active` |
| Line height | 18 px fixed | 20 px, as a token constant until Phase 12 turns it into a setting |
| Gutter width | `digits * 7 + 11` (`:949`) | Measured from the mono font's digit advance so it stays correct at every font size |
| Diagnostic markers | 2.5 px dot | 6 px rounded bar at the gutter's inner edge, semantic color |

**Tests:** selection fill differs from the editor background by at least 3:1; the active-line fill is nonzero and composites above the background; wrapped continuation galleys start at a greater x than their first row; indent guides are emitted only for visible lines.

**Done:** dragging a selection across a paragraph is unmistakable; the caret's line is locatable without moving the eye to the gutter.

### Phase 5 — The file tree

**Changes to `src/tree_surface.rs`:** row height 26 → 28 comfortable / 24 compact from the density setting; folder glyph becomes `text.muted` (accent only on the selected row); the disclosure chevron moves off the indent-guide column so the two stop overlapping; guides drawn in `editor.indent.guide` only for depths that have a parent still on screen; labels truncate with a real ellipsis and gain a tooltip; the galley cache keys on `(revision, selected, hovered)` so label color responds to state like every other list in the product; a 28 px root header shows the project name with the folder path as a tooltip — the first time the window says what is open.

**Tests:** the cached galley for a selected row differs from the unselected one; the chevron center and the guide x differ by at least the stroke width; a long label produces a truncated galley narrower than the panel.

### Phase 6 — Tabs, titlebar, and pane header

**Changes to `src/app.rs`:** tabs size to content between 120 and 240 px with left-aligned labels; the per-tab divider disappears on the active tab and its neighbor; the active tab is marked by `surface.editor` fill plus a 2 px accent bar and `ui.strong` text, so it reads as continuous with the document below it; the dirty dot uses `warning` and sits left of the label rather than swapping places with the close control; chrome row heights collapse to 34/30; the find bar drops 38 → 34.

The pane header (`src/app.rs:3967-4124`) becomes the product's status surface, in line with the existing no-status-bar decision: diagnostics counts as semantic-colored pills, the LSP state, the markdown preview toggle, and the reserved slot for the Vim mode pill that `KEYBINDINGS_PLAN.md` specifies.

On macOS, replace the hand-painted traffic lights (`src/app.rs:1887-1902`) with the real `NSWindow` buttons, or at minimum match the system fills, draw the hover glyphs, and render the unfocused gray state.

**Tests:** tab label left edge is stable when a neighboring tab's width changes; a tab narrower than its label still reserves the close hit target; the existing `editor_column_fills_the_window_without_a_statusbar` test continues to pass unchanged.

### Phase 7 — The icon system

**Changes:** one `src/icons.rs` with `enum Icon` and `fn paint(icon, painter, rect, color, size)`. All glyphs are authored on a 16 px grid, drawn at 1.25 px, and use a single `Painter::line` path so joins are mitred instead of two overlapping segments. Migrate the tree chevron and file/folder glyphs, `components::{chevron_icon_button, close_icon_button, paint_checkmark}`, the titlebar toggles, the settings gear and back arrow, the agent header controls, and the terminal glyph. Icon buttons standardize at 28×28 with a radius-6 `state.hover` fill. The tab close `×` becomes a real icon rather than a text glyph (`src/app.rs:4554`).

**Tests:** every `Icon` variant paints within its 16 px box; every icon shape uses the shared stroke width.

### Phase 8 — Agent panel

**Changes:** user turns render as a left-accent-railed block on `surface.chrome` rather than inside an input-shaped box; tool activity collapses to a 28 px row — icon, monospace summary, elapsed time, status glyph — with the card treatment reserved for expanded content; diffs use `success.subtle`/`danger.subtle` with a semantic gutter rail; markdown lists gain hanging indent (`src/markdown.rs:69-72` moves the bullet out of the wrapped text run); inline code gets radius 4 and symmetric padding; the composer keeps its border but gains a real 32 px send button using `accent` when the prompt is non-empty and `state.hover` when it is not; the provider selector and the mode dropdowns become segmented chips instead of bare text with chevrons.

**Tests:** a wrapped list item's continuation x exceeds the bullet x; the composer's send control reports `WidgetType::Button` with an enabled/disabled state matching prompt emptiness.

### Phase 9 — Dialogs rebuilt as one modal system

**Goal:** replace seven improvised dialogs with a single component that makes the wrong dialog impossible to write. This phase is a rewrite, not a restyle.

**One builder, one call site shape.** `components::dialog_window` + `begin_dialog` + `dialog_actions` + `dialog_button` collapse into one typed builder that owns the frame, the scrim, the spacing, the button order, and the keyboard:

```rust
match Dialog::new("unsaved_dialog", "Unsaved changes")
    .severity(Severity::Warning)
    .body("Save your changes to config.toml before continuing?")
    .destructive("Discard")
    .primary("Save")
    .show(ctx)
{
    Outcome::Primary => { /* save */ }
    Outcome::Destructive => { /* discard */ }
    Outcome::Cancel | Outcome::Dismissed => { /* restore */ }
    Outcome::Open => {}
}
```

The title is passed once. Cancel exists unless explicitly suppressed. Call sites stop declaring `let mut save_clicked = false;` and reading it back after the closure.

**Fixed anatomy.** Every dialog in the product gets exactly this, with no per-call-site deviation:

| Region | Specification |
| --- | --- |
| Scrim | `surface.sunken` at 45% over the whole window, absorbing pointer events; click outside cancels for non-destructive dialogs only |
| Container | `surface.raised`, radius 14, `border.strong`, `shadow.dialog`, min width 380, max width 520, centered |
| Padding | 20 around; 12 between the title row and the body; 20 between the body and the actions |
| Severity glyph | 20 px icon in the title row from `src/icons.rs`, tinted `info`, `warning`, or `danger`; omitted for neutral prompts |
| Title | `ui.title`, `text.primary`, one line, truncating |
| Body | `ui.body`, `text.secondary`, wrapping; file paths render monospace and middle-truncate so both the directory and the filename survive |
| Actions | Right-aligned, 8 px gap, 32 px tall, uniform width floor of 84; order is destructive, then cancel, then primary |
| Keyboard | Enter runs the primary action, Esc cancels, Tab cycles, initial focus goes to the first text field or else the primary action |
| Focus | 2 px `border.focus` ring on the focused control, which no dialog draws today |
| Motion | Scrim fades and the container scales 0.98 → 1.0 over `motion.base` once Phase 11 lands |

**Button variants.** `dialog_button` gains `Variant::{Primary, Neutral, Destructive}`. Destructive is `danger` text on `danger.subtle` with a `danger` border at 35%, and it inherits the same height and width floor as its neighbors, which deletes the hand-built Discard button at `src/app.rs:11568-11574`.

**Errors split into two surfaces.** The `error_banner` modal (`src/app.rs:6843-6858`) is replaced by both of the surfaces it is currently pretending to be. Failures that need no decision become a **toast**: a bottom-right stack on `surface.raised` with a 3 px severity rail, `ui.body` text, an optional single action such as Retry or Reveal, auto-dismissing after 6 seconds, pausing while hovered or focused, at most three visible with older ones collapsing into a count. Failures that need a decision become an alert built on `Dialog`. This is the only new surface the plan introduces, and it earns its place by removing the modal interruption from every save error, LSP failure, and agent transport error.

**Per-dialog corrections:**

| Dialog | Today | After |
| --- | --- | --- |
| Unsaved changes | `[Cancel][Discard][Save]`, custom Discard, only dialog with body spacing | Warning severity, standard order, destructive variant, body names the file |
| File changed on disk | Three equal-weight buttons; body never says what Reload costs | Warning severity, body names the file and states that reloading drops local edits, Reload primary |
| Save As | Bare `text_edit_singleline`, no focus, no validation | Focused on open, monospace path field, inline error when the parent directory is missing, Save disabled while empty, explicit overwrite confirmation |
| Save before running Agent | Body opens with the provider name mid-sentence | Provider name and mark in the title row, body names the buffer |
| Error | Modal banner with hardcoded red body text | Toast or alert chosen by severity |
| Agent file picker | Dialog chrome wrapped around a list (`inner_margin(0)`) | Popover chrome shared with the command palette, not the dialog frame |
| New keybinding profile, Record shortcut | Duplicated titles, manual button wiring | Rebuilt on `Dialog`; Record shortcut gets a live key-capture field with the pressed chord as a chip row |

**Tests:** the builder emits exactly one title galley; Enter yields `Outcome::Primary` and Esc yields `Outcome::Cancel`; the destructive and neutral buttons report equal heights and a shared width floor; a click landing on the scrim never reaches the editor beneath it; a toast expires on schedule, and a hovered toast does not; `Save` stays disabled while the Save As path is empty.

**Done:** no call site outside `src/components.rs` constructs an `egui::Button`, a `Window`, or a `Frame` for a modal; all seven dialogs and every future one come from `Dialog`; a dialog and the command palette read as the same product.

### Phase 10 — Palette, popovers, and empty states

**Changes:** the search palette and every menu adopt `shadow.popover`, radius 10 for popovers and 14 for the palette, `border.strong`, and 20 px insets, so the three overlay species collapse into two — modal and popover. The palette gains grouped result sections with `ui.micro` headers, a result count, and match highlighting on the query substring. The settings rail adopts the same row treatment as the file tree.

The empty editor state (`src/app.rs:9372`) replaces its single weak sentence with a centered block: the Editur mark, the project name, and five keyboard rows — search project, toggle files, open agent, toggle terminal, settings — read from the keybinding catalog in `src/keybindings.rs` so the hints stay true when a user rebinds. This is the cheapest large improvement in the plan; it is the first thing a new user sees.

**Tests:** the empty state renders one row per bound command and omits unbound ones; palette result groups render a header only when the group is non-empty.

### Phase 11 — Motion

**Changes:** implement `theme::motion::animate`; apply `motion.fast` to row, tab, and icon-button hover and press; `motion.base` to dialog, palette, and menu appearance (fade plus 0.98→1.0 scale); `motion.slow` to sidebar and agent panel width and to the agentic-mode transition. Every animated painter call folds the quantized value into its retained revision.

**Tests:** `animate` reaches its target within the stated duration and is stable afterward; two distinct in-flight values produce two distinct retained revisions; an idle frame requests no repaint.

**Done:** `cargo run --release --example benchmark_resize` shows no regression in the median frame; idle CPU stays at or below 0.1%.

### Phase 12 — Appearance settings and the second theme

**Changes:** add an `appearance` section to `Settings` (`src/settings.rs:16`) with `theme` (dark | light | system), `density` (comfortable | compact), `editorFontFamily`, `editorFontSize`, `lineHeight`, and `reducedMotion`. Add the Appearance page to the settings rail beside Language Servers. Make `Theme` a runtime value rather than a set of `const`s so a second palette becomes possible, then author the light theme against the same token names, with the same contrast test suite applied to the light surfaces.

**Tests:** settings round-trip including unknown-key tolerance; both palettes satisfy the same contrast assertions; theme switching does not require a restart.

## 7. Testing strategy

Following `AGENTS.md`: test the invariants, not the taste.

**Worth testing:** contrast ratios and elevation steps (they are objective and they are what regress); the source-scan test that keeps colors in the theme module; the geometry facts that broke before (chevron versus guide overlap, wrapped-line indent, tab label stability, gutter width against font metrics); state-dependent caching (the tree galley bug); motion's retained-revision folding; settings round-trips.

**Not worth testing:** the specific value of any token beyond its structural relationships, per-widget snapshot images, that a label renders its own text, or that a color constant equals itself. Existing shape-inspection tests such as `directory_row_glyphs_and_label_share_one_vertical_center` (`src/tree_surface.rs:422`) are the right model: they assert a relationship a human would notice, using the tessellated output.

**Visual verification:** capture before/after screenshots at a fixed 1180×760 for each phase — empty state, file open, split panes, agent open, agentic mode, palette, settings — and keep them under `docs/design/`. A phase is not done until its pair is captured.

## 8. Performance and renderer constraints

The retained renderer sets hard rules that any visual change must respect.

- Anything painted per row or per line must stay inside the existing visible-range loops (`src/tree_surface.rs:86`, `src/editor_surface.rs:506`). Indent guides, git badges, and diagnostic rails are per-visible-line work, never per-document work.
- Any value that varies between frames — an animation, an opacity, a hover state — must be folded into `mark_retained`, or the cache will serve a stale frame. Quantize before folding.
- Shadows and gradients are meshes; reuse the existing pattern at `src/app.rs:131-161` rather than adding blur passes.
- Font additions are the only meaningful binary cost in this plan. Subset if two faces push past +1 MiB.
- Re-run `benchmark_resize` and `benchmark_highlighting` after Phases 4 and 11 and update `PERFORMANCE.md`. The targets stand: median resize frame ≤ 0.3 ms, idle CPU ≤ 0.1%, binary < 30 MiB.

## 9. Risks, non-goals, and coordination

**Risks.** `src/app.rs` is roughly 19,000 lines and holds most of the UI, so Phases 1, 2, and 6 will touch it broadly; keep each phase a single reviewable commit series and lean on the source-scan test to prove completeness. Changing the elevation ramp changes every screenshot at once, which makes bisecting a visual regression harder — capture the reference set before starting. Motion is the phase most likely to cause a subtle repaint bug; land it last, behind the reduced-motion setting.

**Non-goals.** No new rendering or theming dependency. No user-authored theme files in this pass (the token module is the prerequisite; JSON themes can follow). No status bar. No minimap. No icon font or SVG atlas — hand-painted vectors stay, they simply get one authority. No second window.

**Coordination.** `src/keybindings.rs` is new and uncommitted, and `src/app.rs`, `src/lib.rs`, and `src/lsp/controller.rs` are modified in the working tree. Phase 10's empty state depends on the keybinding catalog, Phase 9 has to rebuild the two dialogs that work is adding, and Phase 6's pane header shares space with the Vim mode pill from `KEYBINDINGS_PLAN.md`. Land the keybinding work first, or stub the catalog lookup behind a function that Phase 10 can adopt without rework. The LSP diagnostic surfaces in `LSP_PLAN.md` should adopt the semantic tokens from Phase 1 rather than introducing their own colors.

## 10. Definition of done

The visual work is complete when all of the following hold:

1. No color, radius, font size, spacing value, stroke width, or duration is constructed outside `src/theme/`. Colors are enforced by the source-scan test; the rest is a review checklist item against `theme::metrics`.
2. Sidebar, chrome, and document are distinguishable without relying on a border, and every border is quieter than the surfaces it separates.
3. Cyan appears only on the caret, the active tab, the focused control, the selected row of a focused list, and the primary action in a dialog.
4. Text selection, the active line, the active line number, wrapped-line continuation, and indent guides are all visible in a screenshot at 100%.
5. The interface uses two text weights and one type scale, with no size that is not on the scale.
6. Every glyph in the product is drawn by `src/icons.rs` at one stroke width on one grid.
7. Every modal comes from one `Dialog` component with a scrim, a fixed button order, Enter and Esc bound, and a destructive variant; non-blocking errors arrive as toasts rather than modals.
8. Opening the app on an empty project shows the project name and the five primary shortcuts.
9. Hover, press, dialogs, and panel toggles animate, with reduced motion available and idle CPU unchanged.
10. Appearance settings persist theme, density, editor font, size, line height, and reduced motion.
11. `PERFORMANCE.md` is updated with the post-change binary size and resize benchmark, and the before/after screenshot set is committed under `docs/design/`.

## 11. Sequencing

| Order | Phase | Perceived impact | Risk |
| --- | --- | --- | --- |
| 1 | 0 — Token module | None | Low |
| 2 | 1 — Color migration | Low | Medium (breadth) |
| 3 | 2 — Surfaces and borders | High | Medium |
| 4 | 4 — Editor surface | Highest | Medium |
| 5 | 3 — Typography | High | Low |
| 6 | 6 — Tabs and chrome | High | Medium |
| 7 | 5 — File tree | Medium | Low |
| 8 | 7 — Icon system | Medium | Low |
| 9 | 9 — Dialogs | High | Medium (rewrite, but self-contained) |
| 10 | 10 — Palette, popovers, empty states | Medium | Low |
| 11 | 8 — Agent panel | Medium | Low |
| 12 | 11 — Motion | Medium | High |
| 13 | 12 — Appearance settings and light theme | Medium | Medium |

Phases 0 through 4 are the minimum set worth shipping together; they convert the interface from "a good dark theme" into a coherent one. Everything after that compounds.
