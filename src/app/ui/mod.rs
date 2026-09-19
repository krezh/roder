//! The shell-agnostic UI layer: everything both the desktop shell
//! (`app::overlays`, `app::components`) and the mobile shell (`app::mobile`)
//! need in common.
//!
//! The split matters architecturally and is enforced by
//! `tests/mobile_gui_boundary.rs`: mobile may not reach into `app::overlays`,
//! so any state or view a dialog shares between the two shells belongs here,
//! and each overlay module holds only its own desktop rendering.
//!
//! Dialog *state* is grouped by the dialog it belongs to ([`confirm`],
//! [`delete`], [`toast`], [`sweep`]). The rest is machinery several of them
//! share: the open/close lifecycle hooks and focus trap ([`overlay`]), the
//! fuzzy matcher the palettes rank with ([`fuzzy`]), and the palette filters
//! built on it ([`filter`]).

pub(crate) mod confirm;
pub(crate) mod delete;
pub(crate) mod filter;
pub(crate) mod fuzzy;
pub(crate) mod overlay;
pub(crate) mod staleness;
pub(crate) mod sweep;
pub(crate) mod toast;

pub(crate) use filter::{filter_kinds, filter_namespaces};
pub(crate) use fuzzy::highlight;
pub(crate) use overlay::{use_bool_overlay, use_dialog_focus, use_option_overlay};
pub(crate) use staleness::StalenessRing;
