// renovate: depName=@xterm/xterm
pub const XTERM_VERSION: &str = "6.0.0";

// renovate: depName=@xterm/addon-fit
pub const ADDON_FIT_VERSION: &str = "0.11.0";

pub static XTERM_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/xterm.css"));
pub static XTERM_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/xterm.js"));
pub static XTERM_ADDON_FIT_JS: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/xterm-addon-fit.js"));
