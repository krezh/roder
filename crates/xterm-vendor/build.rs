use std::fs;
use std::io::Read;
use std::path::PathBuf;

use flate2::read::GzDecoder;

fn main() {
    let src = fs::read_to_string("src/lib.rs").expect("xterm-vendor: missing src/lib.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");

    let xterm_version = parse_version_const(&src, "XTERM_VERSION").expect("XTERM_VERSION");
    let addon_fit_version =
        parse_version_const(&src, "ADDON_FIT_VERSION").expect("ADDON_FIT_VERSION");

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    fetch_and_extract(
        "@xterm/xterm",
        &xterm_version,
        &[("css/xterm.css", "xterm.css"), ("lib/xterm.js", "xterm.js")],
        &out,
    );
    fetch_and_extract(
        "@xterm/addon-fit",
        &addon_fit_version,
        &[("lib/addon-fit.js", "xterm-addon-fit.js")],
        &out,
    );

    println!("cargo:warning=xterm-vendor: fetched @xterm/xterm@{xterm_version} + @xterm/addon-fit@{addon_fit_version}");
}

fn parse_version_const(src: &str, name: &str) -> Option<String> {
    let needle = format!("pub const {name}: &str = \"");
    let start = src.find(&needle)? + needle.len();
    let rest = &src[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn fetch_and_extract(pkg: &str, version: &str, entries: &[(&str, &str)], out: &PathBuf) {
    let basename = pkg.rsplit('/').next().unwrap_or(pkg);
    let url = format!("https://registry.npmjs.org/{pkg}/-/{basename}-{version}.tgz");

    let bytes = reqwest::blocking::get(&url)
        .unwrap_or_else(|e| panic!("xterm-vendor: fetching {url}: {e}"))
        .error_for_status()
        .unwrap_or_else(|e| panic!("xterm-vendor: {url} returned {e}"))
        .bytes()
        .unwrap_or_else(|e| panic!("xterm-vendor: reading {url} body: {e}"));

    let mut archive = tar::Archive::new(GzDecoder::new(bytes.as_ref()));
    let mut found: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for entry in archive.entries().expect("xterm-vendor: malformed tar") {
        let mut entry = entry.unwrap_or_else(|e| panic!("xterm-vendor: tar entry from {url}: {e}"));
        let path = entry
            .path()
            .unwrap_or_else(|e| panic!("xterm-vendor: tar path from {url}: {e}"));
        let path = path.to_string_lossy().to_string();
        let inner = path
            .strip_prefix("package/")
            .map(|s| s.to_string())
            .unwrap_or(path);
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .unwrap_or_else(|e| panic!("xterm-vendor: reading {inner} from {url}: {e}"));
        found.insert(inner, buf);
    }

    fs::create_dir_all(out).unwrap_or_else(|e| panic!("xterm-vendor: creating OUT_DIR: {e}"));
    for (in_name, out_name) in entries {
        let bytes = found
            .get(*in_name)
            .unwrap_or_else(|| panic!("xterm-vendor: {pkg}@{version} missing `package/{in_name}`"));
        fs::write(out.join(out_name), bytes)
            .unwrap_or_else(|e| panic!("xterm-vendor: writing {out_name}: {e}"));
    }
}
