# Resource Tree visual design

## Context

The Resource Tree feature (right-click a Kustomization/HelmRelease → "Resource Tree") is implemented and working — backend resolution (`crates/k8s/src/backend/tree.rs`, `helm_release.rs`), the `/api/resource-tree` route, and a first-pass frontend (`src/app/overlays/tree.rs`) rendering a plain-text tree with ASCII branch connectors (`├─`/`└─`/`│`).

After using it against a real cluster (135 Kustomizations under one root `cluster-apps`), the plain-text rendering read as "barebones" — a debug dump rather than a polished UI element. This spec covers a visual and interaction overhaul of the rendering layer only; the backend data model (`ResourceTreeNode`, the recursive resolution, the `/api/resource-tree` endpoint) is unchanged except for one additive field (`category`).

Reached through iterative mockup review (via the brainstorming visual companion) against real data from the user's cluster, explicitly rejecting a full ArgoCD-style free-form pan/zoom graph as too large a lift for the value — this keeps the existing recursive vertical/indented structure, replacing only how each row is drawn.

## Icon & color system

Reuses `roder_core::Category` (`Workloads`, `Config`, `Network`, `Storage`, `Rbac`, `Flux`, `ExternalSecrets`, `CertManager`, `Rook`, `Cluster`, `Custom`) — the same classification already driving the sidebar's grouped sections — rather than a new per-Kind taxonomy. `ResourceTreeNode` gains `pub category: Option<Category>`, populated for free during backend resolution (every place a node's `key` is resolved against the catalog already has the matching `CatalogEntry`, which carries `.kind.category`); `None` only in the existing "kind not in catalog" case, same as `key: None` today.

Each `Category` maps to a small icon chip: a background/foreground color pair plus a hand-drawn inline SVG glyph, following the existing `ShiftIcon` precedent in `src/app/components/icons.rs` (no new icon-library dependency). A generic fallback glyph/color covers `None`/`Custom`. This is the first per-kind iconography anywhere in the app, deliberately scoped to Category (a dozen buckets) rather than per-Kind (hundreds), to stay maintainable.

Known accepted trade-off: Kustomization and HelmRelease share one Flux color (both `Category::Flux`), and e.g. ConfigMap/Secret share one Config color — distinguished instead by icon *shape* (glyph differs per specific kind within a category where it matters, e.g. Kustomization vs HelmRelease), not by giving every kind its own color.

## Row treatment

Replaces the current ASCII branch-line rendering entirely — hierarchy is now conveyed by indentation (a left border guide per nesting level, as today) plus the row's own shape, not literal `├─`/`└─`/`│` characters.

**Owner nodes** (Kustomization/HelmRelease — anything with `status: Some`/children):
- Fixed-width (280px) rounded card, not stretched to fill the row.
- Border color follows the node's status (reusing the existing `ok`/`warn`/`error`/`pending`/`unknown` palette from `dot_class` — not just green) — this replaces the earlier separate status-dot idea; the border itself is the status indicator.
- Icon chip (20×20), then a stacked two-line block: name (bold), kind + namespace below it in muted text (e.g. "Kustomization · flux-system").
- A right-aligned trailer group, sized to travel together so neither element gets crowded out by the other: when collapsed, a bold descendant-count pill (bordered badge, e.g. "18") followed by a chevron (▶); when expanded, just the chevron (▼). Count and chevron must share one flex group with a single `margin-left: auto`, not two independent auto-margins, or the count pushes the chevron out.

**Leaf nodes** (everything else): a compact chip, *not* a full card — content-sized (not stretched, not fixed-width), multiple per line via a wrapping flex container (`flex-wrap`) at each nesting level. Structurally identical to the owner card's content block: smaller icon chip (16×16), then name (bold) stacked above kind in muted plain text — no pill/badge/border around the kind label, no status color. This wrapping is what actually trades horizontal space for vertical space: the numerous leaf resources pack several-per-line instead of one-per-line, while the few owner cards keep their fixed single-column width since they need room for the trailer and benefit from consistent scanning position.

## Collapse / expand

Owner cards are clickable to toggle their own children's visibility; state is per-node, client-side only (the tree is refetched fresh each time the window opens, so no persistence is needed across opens).

**Behavior change from the first-pass implementation:** previously every row (including Kustomization/HelmRelease) opened the detail drawer on click. Owner cards now use their click for toggle instead — there's no separate click zone for "open this Kustomization's own detail" from within the tree (every mockup reviewed showed the whole card as the toggle target, with no alternate affordance). Leaf chips are unaffected and still open the detail drawer on click. If this trade-off turns out to matter in practice, a follow-up could add a small "open" icon/button inside the card as a second click target — not needed for this pass.

Default rule: **only the root node starts expanded — every other owner node, at any depth, starts collapsed.** This means the root's direct children are always visible (since the root itself is expanded), but each of *those* children — if they're themselves owners — render as collapsed cards with a count badge until clicked.

An "Expand all" / "Collapse all" toolbar sits at the top of the tree window (below the title, above the root row) and overrides every row's state in one click — implemented as a shared broadcast signal (an epoch/command pair in context) that each row's local expand-state watches via an effect, rather than a central registry of every row's signal.

Descendant count (for the collapsed-state badge) is computed client-side by walking the already-fetched `ResourceTreeNode` tree recursively (`children.len() + sum(children.map(count))`) — no backend/wire change, since the frontend already holds the full tree in memory after the one-shot fetch.

## Non-goals (explicitly out of scope for this spec)

- Free-form pan/zoom graph canvas (the "true ArgoCD" layout) — flagged during brainstorming as a much larger lift (real graph-layout engine + interactive canvas) than the value justifies right now.
- Search/filter box within the tree — considered alongside the toolbar, deferred; can be added later without disrupting this design (the collapse/expand command-broadcast mechanism generalizes to "expand and highlight matches").
- Per-Kind (as opposed to per-Category) icon/color assignment.
