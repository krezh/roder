use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read mobile source directory") {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn mobile_does_not_import_desktop_gui() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mobile = root.join("src/app/mobile");
    let mut files = Vec::new();
    rust_files(&mobile, &mut files);

    let forbidden = [
        "crate::app::components",
        "crate::app::detail",
        "crate::app::logs",
        "crate::app::overlays",
        "crate::app::views",
    ];
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).expect("read mobile source file");
        for (line_number, line) in source.lines().enumerate() {
            if forbidden.iter().any(|prefix| line.contains(prefix)) {
                violations.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(root).unwrap_or(&path).display(),
                    line_number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "mobile imports desktop GUI:\n{}",
        violations.join("\n")
    );
}
