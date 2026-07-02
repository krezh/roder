use leptos::prelude::*;
use roder_core::Category;

/// Shift key arrow icon (inline SVG, no external deps).
#[component]
pub(crate) fn ShiftIcon() -> impl IntoView {
    view! {
        <svg class="key-shift" viewBox="0 0 10 10" fill="currentColor" aria-hidden="true">
            <path d="M5 0L10 5H6.5V10H3.5V5H0Z" />
        </svg>
    }
}

/// CSS class selecting the icon chip's background/foreground color pair for
/// a resource's sidebar `Category` — see `style/_tree.scss` for the `.cat-*`
/// rules. Reuses the same category taxonomy as the sidebar rather than a
/// separate per-Kind color system (roder's first per-kind iconography is
/// deliberately scoped to ~11 category buckets, not hundreds of kinds).
pub(crate) fn tree_icon_class(category: Option<&Category>) -> &'static str {
    match category {
        Some(Category::Flux) => "cat-flux",
        Some(Category::Workloads) => "cat-workloads",
        Some(Category::Network) => "cat-network",
        Some(Category::Config) => "cat-config",
        Some(Category::Rbac) => "cat-rbac",
        Some(Category::Storage) => "cat-storage",
        Some(Category::ExternalSecrets) => "cat-externalsecrets",
        Some(Category::CertManager) => "cat-certmanager",
        Some(Category::Rook) => "cat-rook",
        Some(Category::Cluster) => "cat-cluster",
        Some(Category::Custom(_)) | None => "cat-fallback",
    }
}

/// Glyph for a resource's icon chip. Mostly per-Category, with a few
/// kind-level overrides where the shape carries real meaning even though the
/// color is shared: Kustomization vs HelmRelease (both `Category::Flux`), and
/// anything that holds credentials (Secret/Certificate/ExternalSecret) always
/// gets the lock glyph regardless of which category it lives in.
pub(crate) fn tree_icon_glyph(category: Option<&Category>, kind: &str) -> &'static str {
    if matches!(kind, "Secret" | "Certificate" | "ExternalSecret") {
        return "\u{1F512}"; // 🔒
    }
    match (category, kind) {
        (Some(Category::Flux), "HelmRelease") => "\u{25C6}", // ◆
        (Some(Category::Flux), _) => "\u{25A3}",             // ▣
        (Some(Category::Workloads), _) => "\u{25A2}",        // ▢
        (Some(Category::Network), _) => "\u{21C4}",          // ⇄
        (Some(Category::Config), _) => "\u{25A4}",           // ▤
        (Some(Category::Rbac), _) => "\u{25C8}",             // ◈
        (Some(Category::Storage), _) => "\u{26C1}",          // ⛁
        (Some(Category::CertManager), _) => "\u{1F512}",     // 🔒
        (Some(Category::ExternalSecrets), _) => "\u{1F512}", // 🔒
        (Some(Category::Cluster), _) => "\u{2B21}",          // ⬡
        (Some(Category::Rook), _) => "\u{2B22}",             // ⬢
        _ => "\u{25CF}",                                     // ●
    }
}

/// Small colored chip showing a resource's category-derived icon. Used by the
/// Resource Tree; `small` selects the leaf-chip size (16px) vs. the owner-card
/// size (20px) — see `.tree-icon`/`.tree-icon-sm` in `style/_tree.scss`.
#[component]
pub(crate) fn TreeKindIcon(category: Option<Category>, kind: String, small: bool) -> impl IntoView {
    let class = tree_icon_class(category.as_ref());
    let glyph = tree_icon_glyph(category.as_ref(), &kind);
    let size_class = if small {
        "tree-icon tree-icon-sm"
    } else {
        "tree-icon"
    };
    view! { <span class=format!("{size_class} {class}")>{glyph}</span> }
}

#[cfg(test)]
mod tree_icon_tests {
    use super::*;
    use roder_core::Category;

    #[test]
    fn flux_kustomization_and_helmrelease_share_color_but_differ_in_glyph() {
        assert_eq!(tree_icon_class(Some(&Category::Flux)), "cat-flux");
        assert_ne!(
            tree_icon_glyph(Some(&Category::Flux), "Kustomization"),
            tree_icon_glyph(Some(&Category::Flux), "HelmRelease"),
        );
    }

    #[test]
    fn secret_bearing_kinds_get_the_lock_glyph_regardless_of_category() {
        let lock = tree_icon_glyph(Some(&Category::Config), "Secret");
        assert_eq!(
            lock,
            tree_icon_glyph(Some(&Category::CertManager), "Certificate")
        );
        assert_eq!(
            lock,
            tree_icon_glyph(Some(&Category::ExternalSecrets), "ExternalSecret")
        );
        assert_ne!(lock, tree_icon_glyph(Some(&Category::Config), "ConfigMap"));
    }

    #[test]
    fn none_category_falls_back() {
        assert_eq!(tree_icon_class(None), "cat-fallback");
    }

    #[test]
    fn custom_category_falls_back() {
        assert_eq!(
            tree_icon_class(Some(&Category::Custom("example.com".into()))),
            "cat-fallback"
        );
    }
}
