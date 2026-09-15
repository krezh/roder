//! One classification of table column headers, shared by every row renderer.
//!
//! The desktop grid (`components::kind_table`), the multi-kind search grid
//! (`views::search`) and the mobile card list (`table_logic::surfaced_cells`)
//! each need to know what a column *means* — identity, a state pill, a literal
//! boolean, a saturation percentage, a live metric — before deciding how to
//! paint it. Holding that knowledge here means a new column is classified once
//! instead of three times, and the three renderers cannot disagree about a
//! header the way they previously did over `Attached`.

/// What a column header means, independent of how any one view paints it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ColumnKind {
    /// The resource's name — rendered by `NameCell`, never flashed.
    Name,
    Namespace,
    /// A timestamp humanized against the clock tick rather than shown raw.
    Age,
    /// Free-text state, painted as a pill in the row's status colour.
    Status,
    /// A literal `true`/`false`, painted by the value itself — green for
    /// "true", amber for "false" — rather than by the row's status.
    Bool,
    /// Usage as a percentage of the pod's request/limit: tinted by value
    /// (see `util::color::pct_thresh_color`) and drawn with a saturation bar.
    /// Also a metric, so it carries a trend arrow and never flashes.
    Percent,
    /// A live metric value — carries a trend arrow and never flashes, so a
    /// scrape that moves every number doesn't strobe the whole table.
    Metric,
    /// Anything else: plain text, flashed on change.
    Plain,
}

impl ColumnKind {
    /// Whether the column summarises the resource's state. Decides which
    /// column a mobile card surfaces, and which cells paint as a status pill.
    pub(crate) fn is_state(self) -> bool {
        matches!(self, Self::Status | Self::Bool)
    }

    /// Whether the column is row identity rather than resource data. These are
    /// rendered by dedicated cells and are never surfaced as a card summary.
    pub(crate) fn is_identity(self) -> bool {
        matches!(self, Self::Name | Self::Namespace | Self::Age)
    }
}

/// Classify a column header. Arms are disjoint apart from `Percent`, which is
/// a narrower case of `Metric` and so is matched first.
pub(crate) fn column_kind(column: &str) -> ColumnKind {
    match column {
        "Name" => ColumnKind::Name,
        "Namespace" => ColumnKind::Namespace,
        "Age" => ColumnKind::Age,
        "Phase" | "Status" | "Ready" => ColumnKind::Status,
        "Mount" | "Attached" => ColumnKind::Bool,
        "%CPU/R" | "%CPU/L" | "%MEM/R" | "%MEM/L" => ColumnKind::Percent,
        _ if column.starts_with("CPU") || column.starts_with("MEM") || column.starts_with('%') => {
            ColumnKind::Metric
        }
        _ => ColumnKind::Plain,
    }
}

/// Colour class for a [`ColumnKind::Bool`] cell, from the cell's own value.
pub(crate) fn bool_color(value: &str) -> &'static str {
    match value {
        "true" => "ok",
        "false" => "warn",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_columns_are_classified() {
        assert_eq!(column_kind("Name"), ColumnKind::Name);
        assert_eq!(column_kind("Namespace"), ColumnKind::Namespace);
        assert_eq!(column_kind("Age"), ColumnKind::Age);
        for column in ["Name", "Namespace", "Age"] {
            assert!(column_kind(column).is_identity());
        }
    }

    #[test]
    fn state_columns_are_classified() {
        for column in ["Phase", "Status", "Ready"] {
            assert_eq!(column_kind(column), ColumnKind::Status);
            assert!(column_kind(column).is_state());
        }
    }

    /// The regression this module exists for: `Attached` and `Mount` are both
    /// literal true/false columns, and every renderer must agree on that.
    #[test]
    fn boolean_columns_are_classified() {
        for column in ["Mount", "Attached"] {
            assert_eq!(column_kind(column), ColumnKind::Bool);
            assert!(column_kind(column).is_state());
        }
        assert_eq!(bool_color("true"), "ok");
        assert_eq!(bool_color("false"), "warn");
        assert_eq!(bool_color("n/a"), "unknown");
    }

    #[test]
    fn saturation_percentages_outrank_the_generic_metric_prefix() {
        for column in ["%CPU/R", "%CPU/L", "%MEM/R", "%MEM/L"] {
            assert_eq!(column_kind(column), ColumnKind::Percent);
        }
    }

    #[test]
    fn metric_prefixes_are_classified() {
        for column in ["CPU", "MEM", "%Idle"] {
            assert_eq!(column_kind(column), ColumnKind::Metric);
        }
    }

    #[test]
    fn unknown_columns_are_plain() {
        for column in ["Capacity", "Volume", "Node", "IP", "Schedule"] {
            assert_eq!(column_kind(column), ColumnKind::Plain);
            assert!(!column_kind(column).is_state());
            assert!(!column_kind(column).is_identity());
        }
    }
}
