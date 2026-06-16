# Dropwire — Design System & Interaction Spec

> Status: **design v1 (next-gen redesign)**, 2026-06-16. Owner: product design / design engineering.
> Scope: the **desktop** app UI/UX (Tauri WebView2/Chromium, ~980×680, resizable, **dark-first**).
> Pairs with [`BRAND.md`](../branding/BRAND.md) (identity), [`ARCHITECTURE.md`](../ARCHITECTURE.md) (the build),
> and the current shipping UI in [`ui/`](../ui/) (`index.html`, `app.css`, `app.js`).
>
> **Hard contract preserved.** This redesign changes *pixels and motion only*. It keeps the exact flow
> (Send · Receive · History · Settings), every Tauri command (`start_send`, `start_receive`,
> `cancel_transfer`, `list_transfers`, `pick_paths`, `pick_dest_dir`, `qr_svg`, `reveal_path`,
> `my_endpoint_id`), and the `Progress` event shape (`kind: importing|ready|peerJoined|transferring|done|error|cancelled`).
> Every DOM id the current `app.js` reads/writes is retained (see [§7 Migration map](#7-migration--keeping-the-contract)).
>
> **How to read this doc:** §1 is the *why*. §2 is the token sheet — copy it into `:root`. §3–§5 are the
> *what* (components, signature motion, screens). §6 is non-negotiable (a11y/perf). §7 maps it onto the
> existing code. Every value here is final and CSS-ready; an engineer should not have to invent a number.

---

## 0. Table of contents

1. [Design direction](#1-design-direction)
2. [Design system (tokens)](#2-design-system--tokens)
3. [Component specs](#3-component-specs)
4. [Signature interactions & animations](#4-signature-interactions--animations)
5. [Layout & screens](#5-layout--screens)
6. [Accessibility, reduced-motion & performance](#6-accessibility-reduced-motion--performance)
7. [Migration — keeping the contract](#7-migration--keeping-the-contract)

---

## 1. Design direction

### 1.1 The one idea: **"The Wire is alive."**

Every other transfer tool draws a **progress bar** — a rectangle that fills. Dropwire draws a **live wire** —
a single luminous line that connects two nodes and *carries current* while bytes move. That line is the
spine of the entire product: it is the logo, it is the loading state, it is the progress meter, it is the
success animation, it is the navigation indicator. **One signature object, used everywhere, that the user
learns to read in three seconds.** When nothing is happening the wire is dim and still. When a peer connects,
it *ignites*. When bytes flow, current visibly travels along it. When the transfer completes, the current
reaches the far node and the node *sparks*. The metaphor in BRAND.md ("a private wire between your devices")
stops being a tagline and becomes the literal interface.

This is the ownable thing. Nobody else in the category (WeTransfer, AirDrop, Snapdrop, LocalSend) has a
signature kinetic object. Dropwire's is the **current-carrying wire**, rendered in the brand's electric lime
against near-black. It is to Dropwire what the radar sweep is to a sonar, what the equalizer is to a music
app: instantly legible, impossible to confuse with anyone else.

### 1.2 The feel (one paragraph)

Opening Dropwire should feel like powering on a precise piece of hardware in a dark room — a Teenage
Engineering device, a visionOS panel, a Linear command surface. The canvas is deep near-black, almost
weightless; surfaces are defined by a faint **surface ladder** and 1px hairlines rather than heavy shadows
(à la Raycast). There is exactly **one** bright thing on screen at a time — the live-wire lime — and it
*earns* its glow by meaning "current is flowing here." Type is confident and quiet: a wiry geometric display
for the few big moments, a clean UI sans for everything else, a generous mono for the one sacred string (the
code). Motion is **physical but disciplined**: things arrive with a soft spring settle, never a bounce-y
cartoon; current flows at a constant believable speed; nothing moves that doesn't mean something. The whole
thing is fast, silent, and inevitable — the user's gut reaction is *"oh, this is the last one of these I'll
ever need."*

### 1.3 Design principles (5)

1. **One live wire, everywhere.** The current-carrying line is the single signature element. Reuse it for
   logo, nav indicator, connection viz, progress, and success. Never introduce a second "hero" visual idiom.
   If you're tempted to draw a generic spinner or bar, draw a segment of the wire instead.

2. **Lime is current, not paint.** The accent appears *only* where something is live: the primary action,
   the focused field, the active nav, the flowing wire, the spark. One — at most two — lime moments per view.
   Everywhere else is near-neutral. Lime glow = "energy is here right now." This makes the eye go exactly
   where the system wants it, and keeps the app from looking like a toy.

3. **Surfaces by light, not by shadow.** Depth comes from a *surface ladder* (each layer a step lighter)
   plus 1px hairline borders and the occasional **inner glow** — not drop shadows. Dark-first means dropped
   shadows read as muddy smears; stepped surfaces and crisp hairlines read as engineered.

4. **Motion is meaning.** Every animation answers "what just changed and why." Spatial continuity (the thing
   you clicked becomes the thing you see), anticipation before big reveals, a soft settle on arrival. No
   decorative motion. Honor `prefers-reduced-motion` with *full functional parity* — the app must be equally
   usable and equally legible with motion off.

5. **Quiet confidence, plain words.** Copy stays in the BRAND voice (warm, plain, privacy-first). The UI
   proves the tech; it doesn't shout it. The most important text in the app — the share code — gets the
   most space, the best type, and the calmest treatment. No fear, no hype, no manufactured urgency.

### 1.4 What we borrowed (and how we made it ours)

| Reference | What's great | Dropwire translation |
|---|---|---|
| **Raycast** | Near-black canvas, surface ladder *instead of* shadows, hairline borders, rare saturated accent | Our `--canvas`/`--surface`/`--surface-2`/`--surface-3` ladder + hairlines; lime used as rarely as Raycast uses its blue |
| **Linear** | Snappy, sub-200ms transitions; everything feels instantaneous; restrained palette | Our `--dur-1/2` (120–200ms) for chrome; `--ease-snap` for presses |
| **visionOS / Liquid Glass** | Inner glow, thin luminous borders, depth from light & translucency | Inner-glow elevation token; the lime focus *ring-glow*; the code-card's edge sheen |
| **Family wallet / Arc** | Playful but tasteful "moment" animations that reward attention; spatial continuity | The ticket reveal, the wire-ignite, the success spark — earned moments, not constant motion |
| **Teenage Engineering** | Hardware-precise, monospaced, restrained, every element deliberate | Mono code as hero; the "power-on" intro; tactile press physics |
| **WAAPI `linear()` springs** | Real spring feel, hardware-accelerated, zero runtime deps | Pre-baked `linear()` spring tokens for the settle/overshoot moments |

---

## 2. Design system — tokens

Dark-first. The dark theme is the *design target*; light is a faithful port. Drop this straight into `:root`.
All colors trace back to BRAND.md §5; new tokens (surface ladder, glows, motion) are additive and on-brand.

### 2.1 Color tokens

```css
:root {
  /* ============ DARK (default / design target) ============ */
  color-scheme: dark;

  /* --- Brand / "Live Wire" current --- */
  --wire:            #D2FF3A;   /* primary accent. ONLY where current flows. */
  --wire-press:      #B6E830;   /* pressed/active depth on primary */
  --wire-dim:        #8FB81F;   /* the wire at rest (un-energized line) */
  --wire-ink:        #0E1116;   /* text/icons ON lime fills — NEVER white */
  --wire-glow:       rgba(210,255,58,0.55);  /* glow color for shadows/filters */
  --wire-glow-soft:  rgba(210,255,58,0.16);  /* faint wash / focus halo */
  --wire-trail:      rgba(210,255,58,0.00);  /* transparent end for current gradients */

  /* --- Surface ladder (depth by light, not shadow) --- */
  --canvas:          #0B0E13;   /* app background — a touch deeper than Wire Black for contrast headroom */
  --wire-black:      #0E1116;   /* brand ink / deepest panels */
  --surface:         #131820;   /* cards, panels, the main content "plate" */
  --surface-2:       #1A212B;   /* raised within a card (code chip, inputs at rest) */
  --surface-3:       #232C38;   /* hover/active raised, menu rows */
  --scrim:           rgba(7,9,12,0.66);  /* modal/overlay dim */

  /* --- Borders / hairlines --- */
  --hairline:        #232B36;            /* default 1px border on cards */
  --hairline-soft:   rgba(242,245,239,0.06);  /* faint internal dividers */
  --hairline-strong: rgba(242,245,239,0.14);  /* emphasized edge / top sheen */

  /* --- Text --- */
  --text:            #F2F5EF;   /* primary */
  --text-muted:      #9BA6B2;   /* secondary / labels */
  --text-faint:      #6B7480;   /* tertiary / hints, placeholders */
  --text-on-wire:    #0E1116;   /* = --wire-ink, semantic alias for clarity */

  /* --- Semantic (from BRAND §5, dark shades) --- */
  --success:         #34D77A;   /* done / complete (true green ≠ lime) */
  --direct:          #34D77A;   /* direct connection badge */
  --relayed:         #E0A93A;   /* relayed badge (amber, honest not alarming) */
  --warning:         #E0A93A;
  --error:           #F2664F;
  --info:            #5AA0FF;   /* resuming — the one place blue appears */

  /* --- Glow / energy washes (semantic, low-alpha) --- */
  --success-glow:    rgba(52,215,122,0.45);
  --error-glow:      rgba(242,102,79,0.40);

  /* --- Elevation (see §2.5) --- */
  --elev-1: 0 1px 0 0 var(--hairline-strong) inset, 0 1px 2px rgba(0,0,0,0.40);
  --elev-2: 0 1px 0 0 var(--hairline-strong) inset, 0 8px 24px rgba(0,0,0,0.45);
  --elev-pop: 0 1px 0 0 var(--hairline-strong) inset, 0 16px 48px rgba(0,0,0,0.55);
  --glow-focus: 0 0 0 3px var(--wire-glow-soft);                 /* focus halo */
  --glow-live:  0 0 16px var(--wire-glow), 0 0 2px var(--wire-glow); /* the wire when energized */
  --glow-success: 0 0 20px var(--success-glow);
}

/* ============ LIGHT (faithful port) ============ */
:root[data-theme="light"] {
  color-scheme: light;
  --wire:            #C6F032;
  --wire-press:      #A8D616;
  --wire-dim:        #B8D84A;
  --wire-ink:        #0E1116;
  --wire-glow:       rgba(168,214,22,0.45);
  --wire-glow-soft:  rgba(168,214,22,0.18);
  --wire-trail:      rgba(168,214,22,0.00);

  --canvas:          #FBFCF9;
  --wire-black:      #0E1116;
  --surface:         #FFFFFF;
  --surface-2:       #F2F5EE;
  --surface-3:       #E8ECE2;
  --scrim:           rgba(14,17,22,0.32);

  --hairline:        #DDE2D8;
  --hairline-soft:   rgba(14,17,22,0.06);
  --hairline-strong: rgba(14,17,22,0.12);

  --text:            #0E1116;
  --text-muted:      #5B6470;
  --text-faint:      #8A93A0;
  --text-on-wire:    #0E1116;

  --success: #1F9D55; --direct: #1F9D55; --relayed: #C98A1E;
  --warning: #C98A1E; --error: #D7402B; --info: #2D7FF0;
  --success-glow: rgba(31,157,85,0.30); --error-glow: rgba(215,64,43,0.28);

  --elev-1: 0 1px 2px rgba(14,17,22,0.06), 0 0 0 1px var(--hairline);
  --elev-2: 0 6px 20px rgba(14,17,22,0.10), 0 0 0 1px var(--hairline);
  --elev-pop: 0 16px 48px rgba(14,17,22,0.16), 0 0 0 1px var(--hairline);
  --glow-focus: 0 0 0 3px var(--wire-glow-soft);
  --glow-live:  0 0 14px var(--wire-glow);
  --glow-success: 0 0 18px var(--success-glow);
}

/* Respect OS preference when no explicit theme chosen (dark is default markup) */
@media (prefers-color-scheme: light) {
  :root:not([data-theme="dark"]) { /* mirror the light block above */ }
}
```

> **Where lime is allowed (the whole list):** primary button fill; focused input ring; active nav indicator;
> the energized wire + flowing current; the connect "ignite" pulse; the success spark; the brand glyph's left
> node; the QR's finder-eye recolor (subtle); the drop-zone armed state. **Nowhere else.** If a screen shows
> two lime elements that aren't part of the same wire, remove one.
>
> **Glow budget:** glow (`--glow-live`, drop-shadow filters) is expensive and precious. At most **one glowing
> element animating at a time** per view. Static surfaces never glow.

### 2.2 Typography

Fonts from BRAND §6, with system fallbacks (zero-download path works). Use **tabular figures** on anything
that updates so numbers don't jitter.

```css
:root {
  --font-ui:      "Inter", -apple-system, "Segoe UI Variable", "Segoe UI", Roboto, system-ui, sans-serif;
  --font-display: "Space Grotesk", "Inter", system-ui, sans-serif;
  --font-mono:    "JetBrains Mono", ui-monospace, "Cascadia Code", "SF Mono", Consolas, monospace;
  --tnum: "tnum" 1, "cv01" 1;  /* tabular figures via font-feature-settings */
}
body { font-feature-settings: "calt" 1, "kern" 1, "liga" 1; }
.tabular { font-variant-numeric: tabular-nums; }
```

Modular scale (1.25 major-third-ish, hand-tuned for a 980×680 window). `px / weight / letter-spacing / line-height`:

| Role | Font | Size | Weight | Tracking | Line-height | Use |
|---|---|---|---|---|---|---|
| `display-hero` | Space Grotesk | **40px** | 600 | -1.5px | 1.05 | Intro splash wordmark, the one biggest number |
| `display-1` | Space Grotesk | **28px** | 600 | -0.8px | 1.15 | View titles (Send / Receive / …) |
| `display-2` | Space Grotesk | **22px** | 600 | -0.5px | 1.2 | Section headers, "Done" |
| `title` | Inter | **17px** | 600 | -0.2px | 1.3 | Card titles, file name |
| `body` | Inter | **15px** | 400 | 0 | 1.5 | Default body, descriptions |
| `body-strong` | Inter | 15px | 600 | 0 | 1.5 | Emphasis in body |
| `ui` | Inter | **14px** | 500 | 0 | 1.4 | Buttons, nav, inputs |
| `label` | Inter | **12px** | 600 | 0.6px (uppercase) | 1.3 | Eyebrow labels ("SHARE THIS CODE") |
| `caption` | Inter | **12px** | 400 | 0 | 1.4 | Hints, meta, ETA |
| `micro` | Inter | **11px** | 600 | 0.4px | 1.2 | Badges, pills |
| `code-hero` | JetBrains Mono | **clamp(20px, 4.6vw, 30px)** | 600 | **+1.5px** | 1.35 | **The share code — the most important text in the app** |
| `code` | JetBrains Mono | 14px | 500 | +0.5px | 1.4 | Endpoint id, hashes, monospace fields |

Rules: headlines are **sentence case**, never Title Case. Headlines use Space Grotesk Medium/SemiBold with tight
tracking. The code is *always* mono, generously letter-spaced, large, and `user-select: all`.

### 2.3 Spacing (8pt scale)

```css
:root {
  --sp-0:  0;
  --sp-1:  4px;    /* hairline gaps, icon-to-text micro */
  --sp-2:  8px;    /* base unit */
  --sp-3:  12px;   /* tight stacks */
  --sp-4:  16px;   /* default gap between related items */
  --sp-5:  24px;   /* card padding, group separation */
  --sp-6:  32px;   /* section rhythm */
  --sp-7:  48px;   /* major separation, drop-zone padding */
  --sp-8:  64px;   /* hero breathing room */
  --sp-9:  96px;   /* top-level vertical rhythm (rare) */
}
```
Card interior padding: **24px** (`--sp-5`). Content max-width: **640px** (forms/text), code/QR hero may go to **720px**.
Main content inset: **40px** horizontal / **32px** top (`--sp-7` / `--sp-6`), reduced to 24px below 900px width.

### 2.4 Radii

```css
:root {
  --r-1:  6px;    /* pills, badges, small chips */
  --r-2:  10px;   /* buttons, inputs, list rows */
  --r-3:  14px;   /* cards, panels */
  --r-4:  20px;   /* hero surfaces (code card, drop zone) */
  --r-full: 999px;/* nav pill, route badge, progress track ends */
}
```

### 2.5 Elevation & shadow

Depth is built from **surface stepping + hairline + (rarely) inner glow** — *not* drop shadows. The shadow
tokens exist only to seat floating things (menus, the ticket card on reveal) and are kept tight and dark.

| Token | Purpose | Recipe |
|---|---|---|
| `--elev-1` | Resting card on canvas | top inner sheen + faint 2px shadow |
| `--elev-2` | Raised panel / hovered card | top inner sheen + 24px soft shadow |
| `--elev-pop` | The ticket card on reveal, menus | top inner sheen + 48px shadow |
| `--glow-focus` | Focus ring on any interactive | 3px lime halo (low alpha) |
| `--glow-live` | The energized wire / connected node | dual lime glow (the ONLY animated glow) |
| `--glow-success` | The success spark | green glow, fires once |

The "top inner sheen" (`inset 0 1px 0 var(--hairline-strong)`) is the visionOS/Raycast trick: a 1px lit top
edge that makes a flat surface read as a physical plate catching light. Apply it to all cards and the primary button.

### 2.6 Motion tokens

The soul of the app. Durations are short for chrome, longer (and rarer) for signature moments.

```css
:root {
  /* Durations */
  --dur-0: 80ms;    /* instant feedback: press-down, tiny state flips */
  --dur-1: 140ms;   /* hovers, small enters, nav indicator */
  --dur-2: 220ms;   /* view content enter, panel swap, default */
  --dur-3: 380ms;   /* ticket reveal, card expand, emphasis */
  --dur-4: 640ms;   /* signature moments: wire ignite, success spark */
  --dur-flow: 1100ms; /* one full lap of "current" along the wire (loops) */

  /* Easings — named, with real curves */
  --ease-out:    cubic-bezier(0.22, 1, 0.36, 1);    /* "snap-out": crisp decel, the default for entrances */
  --ease-in-out: cubic-bezier(0.65, 0, 0.35, 1);    /* symmetric, for moves/reorders */
  --ease-in:     cubic-bezier(0.5, 0, 0.75, 0);     /* accel-out, for exits/dismiss */
  --ease-snap:   cubic-bezier(0.34, 1.2, 0.5, 1);   /* slight overshoot — presses & confirmations */

  /* Spring (WAAPI linear() — paste as easing on .animate()). Settle, no cartoon bounce. */
  /* spring(mass 1, stiffness 320, damping 26) ≈ */
  --spring-settle: linear(
    0, 0.026, 0.103, 0.219, 0.363, 0.521, 0.677, 0.819, 0.937, 1.026, 1.083,
    1.111, 1.114, 1.099, 1.073, 1.042, 1.013, 0.989, 0.974, 0.967, 0.968,
    0.975, 0.984, 0.993, 1.0, 1.003, 1.003, 1.002, 1.0);
  /* spring(mass 1, stiffness 210, damping 20) ≈ a bouncier "pop" for the ticket */
  --spring-pop: linear(
    0, 0.02, 0.08, 0.176, 0.299, 0.444, 0.6, 0.756, 0.9, 1.022, 1.116,
    1.178, 1.207, 1.206, 1.18, 1.137, 1.085, 1.03, 0.982, 0.945, 0.922,
    0.913, 0.916, 0.928, 0.946, 0.967, 0.987, 1.0, 1.008, 1.009, 1.005, 1.0);
}
```

> **Generating the springs:** the `linear()` values above are pre-baked from a spring solver (mass/stiffness/
> damping noted inline). If you retune, regenerate with Motion's `springEasing`/`spring-easing` (pre-compiled,
> zero runtime) — *do not* ship a physics library. Sample at ~30 points across the natural duration.

**Choreography philosophy — the three beats.**

1. **Anticipation (before a reveal).** A tiny opposite-direction wind-up: the trigger dips/scales down ~2%
   for `--dur-0` before the thing appears. Used on the pick→ticket moment and button presses. Sells intent.
2. **Stagger (when many things arrive).** Children enter on a **40ms cascade** (`delay = index * 40ms`),
   max ~6 visible items animated, the rest snap. Order follows reading order (top→bottom, left→right). This
   is what makes a list feel "dealt" rather than "dumped."
3. **Settle (on arrival).** Entrances land with `--spring-settle` (one soft overshoot, then rest) — never a
   linear stop, never a big bounce. The wire's current uses *constant* linear timing (a real current doesn't
   ease); everything mechanical/UI uses spring or `--ease-out`.

**Direction grammar:** Send-related motion flows **left→right / upward** (sending out). Receive-related motion
flows **right→left / downward** (coming in). The wire's current always flows **from the local node toward the
peer** on the sender, and **toward the local node** on the receiver — so the animation direction itself tells
you which side you're on.

---

## 3. Component specs

> Convention below: every component lists box (size/padding/radius), color (resting + states), type token,
> and motion. All interactive elements get `--glow-focus` on `:focus-visible` and respect reduced-motion.

### 3.1 Buttons

**Primary (`.btn`) — the "send current" button.** This is a lime fill; it is the loudest thing allowed.

- Box: height **40px**, padding `0 18px`, radius `--r-2`, `font: var(--ui)` 14/600, `gap: 8px` for icon.
- Color: bg `--wire`, text `--text-on-wire`. Top inner sheen `inset 0 1px 0 rgba(255,255,255,0.25)`.
- Resting: a *very* faint `--glow-live` at 30% (`box-shadow: 0 0 12px rgba(210,255,58,0.18)`) so it reads as
  "charged." This is the one resting glow permitted, because it's the primary call to action.
- Hover (`--dur-1`, `--ease-out`): bg unchanged, glow rises to ~50%, `transform: translateY(-1px)`.
- Press (`--dur-0`, `--ease-snap`): `transform: translateY(0) scale(0.97)`, bg `--wire-press`, glow drops.
- Disabled: `opacity: 0.4`, no glow, `cursor: not-allowed`.
- Loading: text swaps to a 3-dot wire pulse (three 4px dots, lime, opacity cycling on 0/0.15/0.3s offsets).

**Ghost (`.btn-ghost`) — secondary.**
- Box: same metrics. bg `transparent`, text `--text`, border `1px solid --hairline`.
- Hover: bg `--surface-2`, border `--hairline-strong`. Press: `scale(0.97)`. No glow ever.

**Quiet/tertiary (`.btn-quiet`)** — used for Cancel, Theme: bg transparent, no border, text `--text-muted`;
hover bg `--surface-2`, text `--text`. For destructive/cancel context, hover text → `--error`.

**Icon button (`.icon-btn`)** — 32×32, radius `--r-2`, transparent; hover `--surface-2`. For copy/reveal.

### 3.2 Inputs & the code-entry field

**Standard input.**
- Box: height **44px**, padding `0 14px`, radius `--r-2`, bg `--surface-2`, border `1px solid --hairline`,
  text `--text` 15/400, placeholder `--text-faint`.
- Focus: border `transparent`, `box-shadow: 0 0 0 2px --wire, var(--glow-focus)` (crisp lime ring + soft halo),
  transition `--dur-1 --ease-out`. The ring is the "this field is live/receiving input" current cue.

**Code-entry field (`#recv-code-input`) — the inbound counterpart to the hero code.**
- Box: full width, height **56px**, padding `0 16px`, radius `--r-3`, bg `--surface-2`, mono 16/500 `+0.5px`.
- A small **wire glyph** sits at the left inside the field (a 2-node mini wire, dim). On valid-looking input
  (non-empty), the glyph's left node lights lime — a tiny "I see a wire" acknowledgement.
- Paste affordance: a ghost "Paste" icon-button at the right; if clipboard contains a `blob…`/`http…`-looking
  string, it pulses once (lime, `--dur-2`) to suggest it.
- On submit-with-error (bad code): a **horizontal shake** — `transform: translateX()` keyframes
  `0, -6px, 5px, -3px, 2px, 0` over `--dur-3` `--ease-in-out`, border flashes `--error`. (Reduced-motion:
  border flashes `--error` + the error text appears, no shake.)
- Caret color: `--wire`.

### 3.3 Cards / panels / surfaces

- Base `.panel`/`.card`: bg `--surface`, border `1px solid --hairline`, radius `--r-3`, padding `--sp-5`,
  box-shadow `--elev-1` (includes the top inner sheen). Hover (for clickable cards) → `--elev-2` + border
  `--hairline-strong`, `transform: translateY(-1px)`, `--dur-1 --ease-out`.
- Hero surfaces (code card, drop zone): radius `--r-4`, padding `--sp-6`.
- Nested raised element inside a card (chip, input): bg `--surface-2`. On hover within a row: `--surface-3`.
- Never stack more than 2 surface levels visually in one component (canvas → surface → surface-2 is the max
  the eye should parse).

### 3.4 Navigation — **rail, not sidebar**

The current 220px sidebar is too heavy for a 4-item, single-purpose app in a 980px window. Replace it with a
**slim 64px icon rail** on the left — pure chrome, all room given to content. (This is the modern minimal nav
for a focused desktop tool: visible, one-click, no labels-clutter, with a moving lime indicator that *is* a
wire segment.)

- Rail: width **64px**, bg `--wire-black` (deepest), full height, `display:flex; flex-direction:column;
  align-items:center; padding: 16px 0; gap: 4px`. A 1px right hairline `--hairline-soft`.
- Brand glyph at top: the 28px wire-mark (logo), `margin-bottom: 16px`. Its left node carries lime.
- Nav items: 44×44 icon buttons, radius `--r-2`, icon 20px, color `--text-faint`. Tooltip label on hover
  (small surface-3 chip to the right, `--dur-1` fade+slide 4px). Hover: color `--text`, bg `rgba(255,255,255,0.05)`.
- **Active indicator = a wire segment.** A **3px-wide, 20px-tall lime bar** with `--glow-live` pinned to the
  *left edge* of the rail, vertically centered on the active icon. The active icon itself goes `--wire`.
  When you switch tabs, this bar **slides** to the new item (`--dur-2`, `--spring-settle`) — it literally
  travels like current down the rail. This is the nav's signature: the indicator is a live wire.
- Foot of rail: theme-toggle icon button + a 8px **serverless status dot** (lime when discovery is up, amber
  if relay-only, with a tooltip). The dot has a slow 3s breathing opacity (0.6→1) — "heartbeat," the only
  ambient motion in chrome (disabled under reduced-motion → static).

> Optional power-user nav: a `⌘K`/`Ctrl-K` command surface is *out of scope for v1* but the rail's restraint
> leaves room for it later (Raycast-style). Note it; don't build it now.

### 3.5 The SHARE-CODE display — **the iconic hero**

This is the single most important surface in the product. It must feel like a *ticket / boarding pass for a
file* — something precious you hand to one person.

- Container: a **boarding-pass card**, max-width 560px, radius `--r-4`, bg `--surface`, `--elev-2`, padding
  `--sp-6`. A faint vertical lime hairline runs down the **left edge** (the card's "wire spine," 2px,
  `--wire-dim`, lighting to `--wire` while serving).
- Eyebrow: `label` token, `--text-muted`, "SHARE THIS CODE".
- The code itself: `code-hero` token, centered, `user-select: all`, color `--text`, with the **node-dots of
  the code grouped** — render the ticket as monospace; if it's a long base32 ticket, the UI may show a
  friendlier short form but the *full* string is what copies (per ARCHITECTURE: don't show raw 52-char as the
  primary affordance — show it on a single line with `text-overflow` and a "show full" expander, but copy the
  whole thing). Letter-spacing +1.5px makes it speakable/typeable.
- The code sits in a `--surface-2` inset slab (radius `--r-3`, padding `--sp-4`) with a 1px top sheen.
- Actions row (`--sp-4` gap): **[Copy code]** (primary, full-confidence) + **[Cancel]** (quiet).
- Copy feedback: button morphs label "Copy code" → "Copied ✓" with the check drawn via stroke-dashoffset
  (`--dur-2`), button briefly flashes a lime ring; reverts after 1.4s. A 1-line live region announces "Code
  copied."
- Status line below (`caption`, `--text-muted`): "Ready — share this code. Keep the app open until it's
  received." Updates live through the send lifecycle (Preparing → Ready → Receiver connected → Sending → Sent).
- **The card's reveal is a signature moment** — see §4.3.

### 3.6 QR treatment

- The QR (from `qr_svg`) renders into a **white tile** (QR needs light quiet-zone for scanners), radius
  `--r-3`, padding 12px, size **168×168** (the SVG fills it). Sits to the right of the code in the send card
  (stacks below on narrow widths).
- Framing: the white tile is wrapped in a 1px `--hairline` "lens" with `--r-4` and 4px gap, on `--surface`.
  Caption below: "Scan to receive" (`caption`, centered).
- **On-brand accent (subtle):** recolor the QR's three **finder eyes** to `--wire-ink` on a faint lime —
  i.e., post-process the SVG so the locator squares carry a hint of the brand without harming scannability
  (keep ≥ 60% contrast vs. white; test with a phone). If the generated SVG isn't easily recolorable, leave
  it pure black/white — scannability wins over branding here, always.
- Reveal: the QR **draws in** — start `opacity:0; scale:0.96; filter:blur(4px)`, settle to clear over
  `--dur-3 --ease-out`, 80ms after the code lands (stagger). Reduced-motion: instant.

### 3.7 The drop zone

- Box: large target, min-height **200px**, radius `--r-4`, bg `--surface`, border **2px dashed --hairline**,
  centered column content, padding `--sp-7`. Cursor pointer; `tabindex=0`.
- Inside: a **live-wire icon** (the 2-node wire glyph at ~48px, dim), title "Drop a file or folder here"
  (`title` token), "or" divider (`caption`, `--text-faint`), then **[Choose a file] [Choose a folder]**.
- **Idle ambient:** the wire glyph's current is *off* (dim). A single faint dot drifts along it every ~4s
  (one particle, `--wire-glow-soft`) — barely-there "the wire is waiting" life. Reduced-motion: static.
- Hover / focus: border → `--wire-dim`, bg `--surface-2`, the wire glyph brightens 1 step.
- **Armed (dragover):** this is the big one. Border → solid `--wire` (2px), bg gets a `--wire-glow-soft` wash,
  the **whole zone scales to 1.015** (`--dur-1 --spring-settle`), and the wire glyph **ignites** — current
  starts flowing along it (full §4.5 treatment, mini). A label swaps to "Release to send." The effect: the
  surface visibly *wants* the file.
- **Drop:** the file "lands" — a quick `scale 1.015 → 0.99 → 1` press (`--dur-2 --spring-pop`), a lime ripple
  emanates once from drop point (radial, `--wire-glow` → transparent, `--dur-3`), then the view transitions to
  the send/ticket state (§4.3). Reduced-motion: skip ripple/scale; go straight to ticket.

### 3.8 PROGRESS — **current along the wire**, not a bar

Reimagine progress as the live wire carrying current. There is no generic bar.

**The wire progress component (`.wire-progress`):**
- An inline **SVG**, full content width × **56px** tall. A single horizontal path (the wire) runs from a
  **left node** (●) to a **right node** (●), with the signature mid-spark kink in the path (echoing the logo).
- **Track (unfilled):** the path stroked in `--wire-dim` at low opacity (0.35), 3px, round caps.
- **Fill (progress):** the *same path* stroked in `--wire` at 4px, with `--glow-live`. Progress = how far the
  fill has advanced along the path, driven by `stroke-dasharray`/`stroke-dashoffset`:
  `dasharray = pathLength`, `dashoffset = pathLength * (1 - progress)`. Animate dashoffset to the new value on
  each `transferring` event (`--dur-2 --ease-out`). The lit portion = bytes transferred.
- **Flowing current (the magic):** *on top of* the fill, a second stroke with a **short dash pattern**
  (`stroke-dasharray: 2 14`) in bright `--wire`, its `stroke-dashoffset` animated continuously
  (`@keyframes flow { to { stroke-dashoffset: -16; } }`, `--dur-flow` linear infinite). This makes a train of
  little light-pulses *travel along the lit wire toward the peer node* — visible, believable current. The flow
  layer is **clipped to the lit length** (or just rides the fill path so it only shows where there's progress).
- **The nodes:** left node = your device (lime, glowing while serving). Right node = the peer: grey until
  `peerJoined`, then it **ignites** (§4.4). The right node fills proportionally / brightens as progress nears 100%.
- **Numbers:** to the right, a `code`-token readout — `42% · 18.4 MB/s · 0:23 left` (tabular, `--text-muted`).
  Percent in `display-2` mono if you want a hero number; speed/ETA in `caption`.
- **States:**
  - *Connecting* (pre-peer): the wire shows an **indeterminate** scan — a single bright pulse sweeps left→right
    repeatedly (`--dur-flow`), nodes dim. "Connecting…".
  - *Transferring*: as above; flow speed is constant (don't tie particle speed to MB/s — it reads as buggy;
    keep current at a steady believable rate, let the *fill length* and the numbers convey speed).
  - *Done*: §4.6 success.
  - *Error*: fill freezes, turns `--error`, flow stops, a small break/gap appears in the wire at the stall
    point. "Transfer interrupted."
  - *Resuming*: the already-have portion shows as a **dim-lit** segment (`--info` tint) and current resumes
    from there — visually honoring the "pick up where it left off" promise.
- Reduced-motion fallback: **no flowing dashes.** The fill still animates length (a clean, calm growing lit
  line — that's fine and still on-brand), nodes change color instantly, numbers update. Fully legible.

> Implementation note: the SVG path is shared by send and receive; only the **flow direction** differs (sender:
> left→right toward peer; receiver: the lit region grows toward the local node, current flows toward you). Use
> the same component, flip a `data-dir` attribute that negates the keyframe offset sign.

### 3.9 The direct / relayed badge

A pill that tells the truth about the path (BRAND: never hide a relay).

- Box: pill, height 24px, padding `0 10px`, radius `--r-full`, `micro` token, `gap: 6px` with a leading dot.
- **Connecting:** bg `--surface-2`, text `--text-muted`, dot pulsing (breathing). Label "connecting".
- **Direct:** bg `color-mix(in srgb, var(--direct) 16%, transparent)`, text `--direct`, solid dot. Label
  "direct". A tiny "lightning" tick. Tooltip: "Straight to their device — fastest."
- **Relayed:** bg `color-mix(in srgb, var(--relayed) 16%, transparent)`, text `--relayed`. Label
  "relayed · a bit slower". Tooltip: "A relay forwards encrypted bytes when a direct path can't be made.
  Still private." Honest, not alarming.
- Transition between states: dot cross-fades + a `--dur-2` width tween (text changes length); never jarring.

### 3.10 History rows

- Row: `.hist-item`, height ~64px, bg `--surface`, border `1px solid --hairline`, radius `--r-2`, padding
  `--sp-4`, `display:grid; grid-template-columns: auto 1fr auto; gap: --sp-4; align-items:center`.
- Left: a **direction wire-glyph** — a tiny 2-node wire, *arrowed by current direction*: sent = current
  pointing right (lime node on left); received = current pointing left (lime node on right). 28px. This reuses
  the signature instead of a generic icon.
- Middle: file name (`title`, ellipsis) over meta (`caption`, `--text-faint`: size · date · status). Status
  "Done" gets a tiny `--success` dot; "Failed" a `--error` dot; "Resumable" an `--info` dot + a small
  **[Resume]** ghost button on the right (re-invokes `start_receive` into the same dest).
- Right: size + a `…` icon-button → menu (Reveal in folder via `reveal_path`, Copy code, Remove from list).
- Hover: bg `--surface-2`, border `--hairline-strong`, the wire-glyph's current flickers once.
- Enter: rows stagger in (40ms cascade, `--ease-out`, fade+translateY 8px) when History opens.

### 3.11 Empty states

- Centered, generous, *alive*. Big dim **wire glyph** (~80px) with a single slow particle drifting along it
  (one ambient dot, `--dur` ~3.5s loop) — the wire is "idle, waiting."
- Headline (`display-2`, `--text`), subline (`body`, `--text-muted`), and a primary action if relevant.
- History empty: "No transfers yet." / "Files you send or receive will show up here — stored only on this
  device." (No CTA; History is passive.)
- Reduced-motion: the particle is hidden; glyph is static. Never a blank box.

### 3.12 Settings rows

- A single `.panel` containing `.setting` rows, each `display:flex; justify-content:space-between;
  align-items:center; padding: --sp-4 0; border-bottom: 1px solid --hairline-soft` (last row no border).
- Left: `label` token name + optional `caption` description beneath. Right: the value / control.
- This-device ID: `code` token, `--text-muted`, `user-select:text`, with a copy icon-button. Truncated middle
  (`abc…xyz`) with full on hover/expand.
- Connection: the serverless status with the same dot as the rail; a one-line plain explanation.
- Controls (default download folder, theme): use ghost buttons / a segmented control (Auto/Light/Dark) sized
  at 28px height, `--surface-2` track, lime active segment text.
- About: "Dropwire · free & open source · built on iroh" + link to dropwire.app (link color `--wire-press` on
  light, `--wire` on dark — links are the one inline lime-text exception, and only as links).

---

## 4. Signature interactions & animations

Each spec'd as **technique + timing + easing**, with a reduced-motion fallback. Use **WAAPI** (`element.animate`)
for orchestrated/JS-driven sequences (springs, the ignite, the success), **CSS transitions/keyframes** for
state-driven micro-interactions (hover, focus, the looping flow), and the **View Transitions API** for view swaps.

### 4.1 App launch / intro — "power-on"

The first 700ms set the tone. Target: feel like a device booting, not a website loading.

1. `0ms`: canvas paints `--canvas` (instant, no flash — set the bg in the HTML root so there's no white flash
   before CSS, critical in WebView2).
2. `0–260ms`: the **wire-mark draws itself** center-screen — the wire path animates via stroke-dashoffset
   (`pathLength→0`), `--ease-out`; left node fades+scales in (`--spring-settle`) as the stroke reaches it,
   lighting lime with `--glow-live`.
3. `200ms`: a **single current pulse** travels the freshly-drawn wire left→right (`--dur-3`), arriving at the
   right node which brightens — "powered on."
4. `420–700ms`: the mark **docks** — it scales down and translates to its home in the top of the nav rail
   (a real position animation, `--dur-4 --spring-settle`), while the app chrome (rail + first view) fades/
   slides in beneath it (`--dur-2`, 30ms stagger: rail, then content). The first view's content does its
   normal enter (§4.2).
5. The whole intro is **skippable**: any keypress/click jumps to docked state instantly.

Reduced-motion: show the docked logo + app chrome immediately with a single `--dur-2` opacity fade. No draw,
no pulse, no docking move.

### 4.2 View transitions (Send ↔ Receive ↔ History ↔ Settings)

Use the **View Transitions API** (`document.startViewTransition(() => swapView())`). It's Baseline as of Oct
2025 and native in WebView2.

- **Default cross-fade + lift:** outgoing view `opacity 1→0` + `translateY 0→-6px` (`--dur-1 --ease-in`),
  incoming `opacity 0→1` + `translateY 8px→0` (`--dur-2 --ease-out`). Set via `::view-transition-old/new(root)`.
- **The nav indicator leads.** Before the content swaps, the lime rail indicator **slides** to the new tab
  (`--dur-2 --spring-settle`) — the eye follows the current to its new home, then the content arrives. This
  spatial cue is what makes navigation feel intentional rather than abrupt.
- **Directional hint:** going "down" the rail (Send→Settings), content enters from below; going "up", from
  above — small (8px) but reinforces spatial model. Set translate sign by comparing tab indices.
- View titles use a shared `view-transition-name` so the title morphs in place rather than cross-fading
  (subtle continuity, Linear-grade).
- Per VT best practice: **name only what benefits** (root + title + the nav indicator). Don't name every node.

Reduced-motion / no-VT support: plain `display` swap with a `--dur-1` opacity fade (the current behavior,
kept as the floor). VT degrades gracefully on its own.

### 4.3 The SEND "ticket reveal" — the payoff moment

When import finishes and `ready` fires with the ticket, the share-code card must feel *minted* — like a
ticket printing.

1. **Anticipation:** the drop-zone / importing state does a 2% scale-down dip (`--dur-0`) as the last byte
   imports.
2. **Swap:** via View Transition, the importing surface gives way to the ticket card. The card enters with
   `--spring-pop` (the bouncier one — this is *the* reward moment): `opacity 0→1`, `translateY 16px→0`,
   `scale 0.94→1`, over `--dur-3`. `--elev-pop` shadow blooms in.
3. **The code "types in":** the mono code reveals **left-to-right** as if printing/transmitting — either a
   per-character reveal (each glyph `opacity 0→1 + translateY 2px`, 18ms stagger) or a wipe mask moving L→R
   (`--dur-3 --ease-out`). The caret/leading edge carries a 1px lime cursor that runs ahead of the text.
4. **The spine lights:** the card's left lime hairline runs from top to bottom (stroke-dashoffset draw,
   `--dur-3`) and settles to `--wire` — the ticket is now "live."
5. `+80ms`: the QR draws in (§3.6). `+120ms`: the action buttons fade up (stagger).
6. The status flips to "Ready — share this code."

Reduced-motion: card appears with a single `--dur-2` opacity fade; code appears whole (no typing); spine is
statically lime; QR appears instantly. The *information* arrives identically.

### 4.4 + 4.5 The CONNECTION / WIRE visualization — **THE magic moment**

This is the heart of the redesign: an animated wire showing two devices linking and bytes flowing. It appears
in the **active transfer area** (both send & receive), replacing any generic bar. Here is the precise build.

**The stage.** An inline SVG, full content width, ~120px tall for the full hero treatment (or the compact
56px `.wire-progress` from §3.8 inside cards). Coordinate system `viewBox="0 0 600 120"`.

**The wire path.** One `<path>` from the left node to the right node with the signature mid-spark kink:
```
M 60 60 H 280 l 18 -22 18 22 H 540   <!-- node L … spark … node R -->
```
Render in **three stacked strokes** sharing this exact `d`:
- **L0 — track:** `stroke: var(--wire-dim); stroke-width: 3; opacity: .3; stroke-linecap: round`.
- **L1 — fill:** `stroke: var(--wire); stroke-width: 4; filter: url(#wireGlow)` (an SVG `feGaussianBlur`
  glow), with `stroke-dasharray = L; stroke-dashoffset = L*(1-progress)`. This is the lit length.
- **L2 — current:** `stroke: #EAFFA0` (a hot near-white-lime); `stroke-dasharray: 2 16`; `stroke-linecap: round`.
  Animated continuously: `animation: flow var(--dur-flow) linear infinite`, where
  `@keyframes flow { from { stroke-dashoffset: 0 } to { stroke-dashoffset: -18 } }`. The little 2px dashes
  are the **current pulses traveling the wire.** (18 = dash+gap, so it loops seamlessly.) Clip L2 to L1's lit
  region (use the same dashoffset trick or an SVG `<clipPath>` that tracks progress) so current only shows on
  the energized part.

**The nodes.** Two `<circle>`s (r=8): **N_self** (your device) and **N_peer**. Each has a small outer ring
`<circle>` for the glow pulse.

**The choreography, beat by beat:**

1. **Idle / connecting** (`ready`→ before `peerJoined`): track visible, fill at 0, both nodes dim grey. A
   single bright pulse (a temporary L2 with one dash) **sweeps the full wire L→R repeatedly** (`--dur-flow`)
   — the system "calling out," searching for the peer. Status: "Waiting for receiver…" / "Connecting…".
2. **Connect (`peerJoined`) — THE PULSE.** A discrete, satisfying handshake:
   - N_peer **ignites**: scales `0.6→1.15→1` (`--spring-pop`, `--dur-4`), color grey→`--wire`, its glow ring
     expands `r 8→26` while fading (`opacity .8→0`) — a single radial "spark" ring.
   - **Simultaneously** a bright current pulse fires from N_self and **races to N_peer** along the wire
     (`--dur-3 --ease-out`); on arrival N_peer's ignite triggers (above) — cause and effect.
   - The whole wire's fill "warms": track opacity .3→.5 for `--dur-2`.
   - A soft tick in the status: "Connected — sending." (Optional: a barely-audible click if audio is ever
     added; not in v1.)
3. **Transferring (`transferring`):**
   - L1 fill length tweens to `progress` on each event (`--dur-2 --ease-out`) — lit wire grows toward N_peer.
   - L2 current flows continuously (constant speed) along the lit region — the visible "bytes moving."
   - N_self glows steadily (`--glow-live`); N_peer brightens as progress → 100%.
   - The route badge (§3.9) sits above the wire showing direct/relayed.
   - Numbers update beside it.
4. **Done (`done`) — completion:** see §4.6.
5. **Error/cancel:** current stops, fill freezes & desaturates to `--error`, a 6px **gap** opens in the wire
   at the lit edge (the "break"), N_peer dims. Status: the error message in plain words.

**Direction grammar (restated):** on the **sender**, current flows N_self→N_peer (left→right, outgoing). On
the **receiver**, the lit region grows from N_peer toward N_self and current flows **toward you** (right→left
by negating the keyframe). The animation direction alone tells you which side you're on.

**Why this technique:** stroke-dashoffset for the lit length is GPU-cheap and exact; a dashed second stroke
for "current" is the canonical, performant flow trick (no particle system, no canvas loop) and loops perfectly.
The only `filter`/glow is on L1/the nodes, kept to one wire at a time → stays 60fps in WebView2.

**Reduced-motion:** **no L2 flow, no sweep, no ignite spring.** Keep: track + a clean fill that animates its
length on progress (calm growing lit line), nodes change color instantly on connect/done, numbers update.
Still unmistakably the wire, fully informative, zero vestibular risk.

### 4.6 The "done" success moment — the wire completes

Tasteful, on-brand, fires **once**. The current reaches the far node and the transfer "lands."

1. The fill completes to 100% (`--dur-2`), the final current pulse **arrives at N_peer**.
2. **N_peer sparks:** a single radial ring (`r 8→34`, `opacity .9→0`, `--dur-4 --ease-out`) in `--success`
   (true green — "complete" ≠ "active"), node turns `--success` with `--glow-success`.
3. The whole wire briefly flushes `--success` (L1 color `--wire`→`--success` over `--dur-3`) then settles to
   a calm success-green resting line.
4. A **checkmark** draws itself (stroke-dashoffset) inside/near N_peer, `--dur-3 --ease-out`.
5. The card's status morphs to "Sent ✓" / "Received ✓ — 18.4 MB in 23s", and (receive) the **[Open folder]**
   button springs in (`--spring-pop`, `--dur-3`) with primary emphasis.
6. **No confetti, no sound, no bounce-fest.** One clean spark. That restraint *is* the premium feel.

Reduced-motion: node + wire turn `--success` instantly, checkmark appears whole, status updates, Open-folder
button fades in. No ring, no flush animation.

### 4.7 Drag-drop choreography

Covered in §3.7; the four states are **idle (waiting particle) → hover (brighten) → armed/dragover (ignite +
scale 1.015 + "Release to send") → drop (press-pop + ripple → ticket)**. The dragover state is wired to
Tauri's `onDragDropEvent` (`enter`/`over` → armed, `leave` → idle, `drop` → land). The CSS class on
`#send-pick` remains `.dragover` (no JS contract change).

### 4.8 Hover / press micro-interactions (global grammar)

- **Hover** (`--dur-1 --ease-out`): interactive surfaces lift `translateY(-1px)` and step one surface level
  (or raise glow, for primary). Cursor `pointer`.
- **Press** (`--dur-0 --ease-snap`): `scale(0.97)` + settle back; primary darkens to `--wire-press`.
- **Focus-visible** (keyboard): `--glow-focus` ring on everything, always. The ring is the lime "current
  found this control" cue. Never remove focus outlines.
- **Toggle/confirm** (copy, theme): `--ease-snap` with the tiny overshoot for a tactile "click" feel.
- All of the above collapse to a **simple color/opacity change** under reduced-motion (no transform).

### 4.9 Connecting / loading states

- **Importing (send, pre-ticket):** a compact wire whose **current sweeps L→R** while a determinate ring/text
  shows hashed bytes ("Preparing… 24 MB / 120 MB"). It's the same wire idiom, not a spinner.
- **Connecting (receive, pre-peer):** the connecting sweep from §4.4 beat 1 + "Connecting…" + the route badge
  in its pulsing "connecting" state.
- **Generic inline busy** (rare): a 3-dot lime wire-pulse (the button loading dots), never an OS spinner.

### 4.10 Empty-state life

Covered in §3.11: a dim wire glyph with one slow drifting current particle. The wire is *idle but powered* —
the product's resting heartbeat. Disabled under reduced-motion.

---

## 5. Layout & screens

Window ~980×680. Layout = **64px nav rail** + fluid content. Content is centered in a max-640px (forms) /
720px (hero) column with generous inset. Dark canvas throughout.

### 5.1 Shell

```
┌────┬─────────────────────────────────────────────────────────┐
│ ▣  │   (view title)                                            │  ▣ = brand wire-mark (top of rail)
│ ─  │                                                           │  ─ = lime active indicator (slides)
│ ↑  │                                                           │  rail icons: Send ↑ / Receive ↓ /
│ ↓  │            content column (max 640–720)                   │  History ⟳ / Settings ⚙
│ ⟳  │                                                           │
│ ⚙  │                                                           │
│    │                                                           │
│ ◐• │                                                           │  ◐ theme · • serverless status dot
└────┴─────────────────────────────────────────────────────────┘
```

### 5.2 Send

- **Title block:** "Send" (`display-1`) + sub "Pick a file or folder. Share the code — it goes straight to them."
- **State A (idle):** the hero **drop zone** (§3.7) centered, ~200px tall, with Choose-file / Choose-folder.
- **State B (importing):** drop zone replaced (View Transition) by a compact importing panel: file name +
  the sweeping importing wire + "Preparing… X / Y".
- **State C (active/ticket):** the **boarding-pass code card** (§3.5) with code + QR side-by-side (stacks on
  narrow), status line, Copy/Cancel. Below it: the **wire-progress** showing waiting → peerJoined → sending →
  done. On `done`: "Sent ✓" and a quiet **[Send another]** that returns to State A.

### 5.3 Receive

- **Title block:** "Receive" + sub "Enter the code you were given. Choose where to save it."
- **State A (entry):** the big **code-entry field** (§3.2) with the mini-wire glyph + paste; below it a
  destination row ("Save to: Downloads · [Choose folder…]") and a full-width primary **[Receive]** (disabled
  until input non-empty). Inline error region under it.
- **State B (active):** file name + **route badge** (connecting → direct/relayed) at top; the **wire-progress**
  (current flowing toward you); numbers (received / total · speed · ETA); **[Cancel]**.
- **State C (done):** "Received ✓ — 18.4 MB in 23s", success-green wire, primary **[Open folder]** (springs
  in) + quiet **[Receive another]**.

### 5.4 History

- Title + sub "Transfers on this device. Stored only here."
- A vertical list of **history rows** (§3.10), newest first, staggered enter. Direction wire-glyphs, names,
  meta, status dots, Resume where applicable, `…` menu (Reveal / Copy code / Remove).
- Empty: the alive empty state (§3.11).

### 5.5 Settings

- Title + sub "Dropwire keeps nothing in the cloud. Everything below lives on this device."
- One panel, `.setting` rows: **This device's ID** (mono, copy), **Default download folder** (path + Change),
  **Connection** (serverless status dot + plain explanation), **Theme** (Auto/Light/Dark segmented),
  **About** (free & open source · built on iroh · dropwire.app link).

---

## 6. Accessibility, reduced-motion & performance

### 6.1 Accessibility

- **Contrast:** all text meets WCAG AA on its surface. Lime is used as a **fill with dark ink** (`--wire` +
  `--text-on-wire` ≈ 13:1, AAA) — **never light text on lime** (BRAND rule). Lime as *text* only appears as
  links / active-icon glyphs on the dark rail where it clears AA against `--wire-black`. Muted text
  (`--text-muted` #9BA6B2 on `--surface` #131820) ≈ 6.3:1 — pass.
- **Focus:** every interactive element has a visible `--glow-focus` ring on `:focus-visible`; never suppressed.
  Logical tab order follows DOM/reading order. The rail is a `<nav>` with `aria-current="page"` on the active
  item; views are `role="tabpanel"`-ish regions with labels.
- **Live regions:** the send/receive status lines and the copy/done confirmations are `aria-live="polite"` so
  the lifecycle (Ready → Connected → Sending → Done) and "Code copied" are announced. Errors `aria-live="assertive"`.
- **The wire viz is decorative** (`aria-hidden="true"`); the *real* status is conveyed in text + the live
  region + the percentage, so a screen-reader user gets full information without the animation. The route
  badge has an accessible label ("Connection: direct" / "Connection: relayed, a bit slower").
- **Keyboard:** all actions reachable; `Enter` submits the code field; `Esc` cancels an active transfer (with
  confirm); `⌘/Ctrl+C` copies the focused code. QR has an `aria-label`; the code is `user-select:all` and
  copyable. Color is never the *only* signal (direct/relayed also differ by label + icon).
- **Hit targets:** ≥ 40×40px for primary controls; rail icons 44×44.

### 6.2 Reduced-motion (full functional parity)

Wrap all decorative/large motion in `@media (prefers-reduced-motion: no-preference)`; provide a calm path in
`@media (prefers-reduced-motion: reduce)`. The reduced experience must lose **zero information**.

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}
```
Then, **selectively re-enable only essential, gentle transitions** (opacity fades ≤ `--dur-2`, the progress
fill *length*) by scoping them outside the blanket rule, so the app still feels responsive rather than dead.
Specifically disabled under reduce: the L2 current flow, the connecting sweep, node ignite springs, the
success ring, the intro draw/dock, drop ripple, ambient particles, button overshoot, view slide (fade only).
Specifically kept: instant color state changes, opacity cross-fades, the determinate fill length, focus rings.
JS gates the WAAPI sequences too: `const RM = matchMedia('(prefers-reduced-motion: reduce)').matches;` — skip
`.animate()` choreography and apply end-states directly when `RM`.

### 6.3 Performance (60fps in WebView2)

- **Animate only `transform` and `opacity`** (and `stroke-dashoffset`, which is compositor-friendly on the GPU
  in modern Chromium). Never animate `width`/`height`/`top`/`left`/`box-shadow`-size/`filter`-radius in loops.
  The progress fill uses `stroke-dashoffset`, not a width-animated div.
- **Glow budget:** SVG `filter`/`feGaussianBlur` and large `box-shadow` glows are the expensive bits — allow
  **one animated glowing element per view** (the active wire). Static glows are fine. Promote the wire SVG with
  `will-change: transform` only while a transfer is active; remove after.
- **The current loop** is a single CSS keyframe on one stroke (`stroke-dashoffset`) — effectively free; it does
  not touch JS per frame. The connecting sweep likewise.
- **No per-frame JS for visuals.** Progress events arrive at a modest rate (Channel-throttled, per ARCHITECTURE
  §7.2); each event sets the target dashoffset and lets CSS/WAAPI interpolate. Do **not** rAF-loop a canvas.
  If a future richer particle field is wanted, gate it behind a single `requestAnimationFrame` with `transform`
  only and a hard 60fps cap — but the dashed-stroke approach above is preferred and avoids canvas entirely.
- **View Transitions:** name ≤ 3 elements (root, title, nav indicator) to keep snapshot layers cheap.
- **No layout thrash:** batch DOM reads/writes; avoid reading `getBBox`/`getComputedStyle` inside event
  handlers that also write. Cache `path.getTotalLength()` once per mount.
- **First paint:** set `--canvas` as the document background in inline `<style>`/root attribute so there is no
  white flash before `app.css` loads (WebView2 will otherwise flash white). Keep total CSS/JS lean — no
  frameworks, consistent with the privacy/no-bloat ethos.
- **Fonts:** `font-display: swap`; ship the 3 brand fonts subset (or rely on the system fallback stack so the
  app is instantly usable offline). Avoid layout shift by sizing fallbacks closely (Inter↔Segoe are close).

---

## 7. Migration — keeping the contract

This redesign is a **CSS + progressive-enhancement-JS** layer over the existing markup. The command contract,
event shapes, and every DOM id in `app.js` are preserved.

**Untouched (do not rename):** the Tauri commands `start_send`, `start_receive`, `cancel_transfer`,
`list_transfers`, `pick_paths`, `pick_dest_dir`, `qr_svg`, `reveal_path`, `my_endpoint_id`; the Channel event
`kind`s `importing|ready|peerJoined|transferring|done|error|cancelled` and their payload fields
(`done/total`, `ticket`, `offset/total/route`, `stats.bytes/seconds`, `message`); the drag-drop `.dragover`
class toggle on `#send-pick`.

**DOM ids kept (so `app.js` selectors keep working):** `#view-send/receive/history/settings`, `.nav-item`
(now rail buttons) with `data-view`, `#send-pick`, `#pick-file`, `#pick-folder`, `#send-active`, `#send-code`,
`#send-qr`, `#copy-code`, `#send-cancel`, `#send-status`, `#send-bar`/`#send-pct` (now the wire-progress fill +
readout), `#recv-code-input`, `#recv-start`, `#pick-dest`, `#recv-dest-label`, `#recv-error`, `#recv-active`,
`#recv-name`, `#recv-route`, `#recv-bar`/`#recv-pct`, `#recv-status`, `#recv-cancel`, `#recv-open`,
`#history-list`, `#history-empty`, `#endpoint-id`, `#theme-toggle`, `#about-link`.

**What changes:**
1. **`app.css`** — replace wholesale with the token sheet (§2) + component styles (§3). Map old vars
   → new: `--accent`→`--wire`, `--accent-strong`→`--wire-press`, `--accent-ink`→`--wire-ink`, `--bg`→`--canvas`,
   `--ink`→`--text`, `--ink-soft`→`--text-muted`, `--ink-faint`→`--text-faint`, `--border`→`--hairline`,
   `--green`→`--success`, `--amber`→`--relayed/--warning`, `--red`→`--error`. The sidebar styles become the
   rail; `.bar/.bar-fill` become the wire-progress SVG (keep the ids on the SVG fill stroke + a `.pct` span).
2. **`index.html`** — restructure the sidebar into the 64px rail; wrap the progress markup in the wire SVG
   (give the fill stroke `id="send-bar"`/`id="recv-bar"` so `setBar()` can still set its progress — adapt
   `setBar` to write `stroke-dashoffset` instead of `width`, the one small JS edit). Add `view-transition-name`
   to titles + root; add `aria-live` to status lines; add the wire SVGs to drop zone / empty states.
3. **`app.js`** — minimal, additive enhancements (all guarded so the app still runs if they're absent):
   - `setBar()`: set `stroke-dashoffset = L*(1-p)` on the fill stroke instead of `width` (cache `L`).
   - Wrap view switching in `document.startViewTransition?.(() => {...})` (fallback to current code).
   - Fire the WAAPI choreography on lifecycle events (`ready`→ticket reveal; `peerJoined`→connect pulse;
     `done`→success spark), each gated by `prefers-reduced-motion` and `typeof Element.prototype.animate`.
   - Slide the rail indicator on nav.
   - None of this changes the command calls or event handling — it only *adds* visual reactions in the same
     handlers (`onSendMsg`/`onRecvMsg`) that already exist.

Net: an engineer ships the new look by swapping the CSS, lightly restructuring the HTML to the rail + SVG
progress, and making `setBar` write a dashoffset — then layers the signature WAAPI moments on top, each
behind a reduced-motion guard. The functional app keeps working at every step.

---

## 8. Quick reference (the one-screen summary)

- **Direction:** *"The Wire is alive."* One signature object — a current-carrying lime wire — is the logo,
  nav indicator, connection viz, progress meter, and success animation.
- **Signature interaction:** the **connection/transfer wire** — `peerJoined` ignites the far node with a
  spark + a current pulse racing across; bytes then flow as a train of light-dashes (`stroke-dashoffset`)
  along the lit wire toward the peer; `done` lands with one green spark.
- **Color:** dark canvas `#0B0E13`; surface ladder `#131820`/`#1A212B`/`#232C38`; hairlines, no shadows;
  **lime `#D2FF3A` only where current flows** (≤ 2 per view), dark ink on lime, glow = "live."
- **Type:** Space Grotesk (display) / Inter (UI) / JetBrains Mono (the hero code, +1.5px tracking).
- **Motion:** durations 80/140/220/380/640ms + 1100ms flow loop; `--ease-out (.22,1,.36,1)`,
  `--ease-snap (.34,1.2,.5,1)`, `--spring-settle`/`--spring-pop` (WAAPI `linear()`); choreography =
  anticipation → 40ms stagger → spring settle; current flows constant-speed and *toward the peer*.
- **Top 5 next-gen levers:** (1) the live-wire progress/connection viz replacing the bar; (2) the boarding-pass
  ticket reveal with the code "transmitting" in; (3) the power-on intro that docks the logo into the rail;
  (4) the sliding lime nav indicator + View-Transition view swaps; (5) surface-ladder + inner-glow depth
  (no shadows) with lime as scarce, meaningful "current." All 60fps, all reduced-motion-safe.
```
