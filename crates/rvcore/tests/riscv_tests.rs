//! Runs the official riscv-tests if they have been built
//! (`make -f tests/riscv-tests.mk`); otherwise the test is skipped.

use std::path::Path;

#[test]
fn riscv_tests() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/riscv-tests");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: {} not built", dir.display());
        return;
    };
    let mut failures = Vec::new();
    let mut count = 0;
    for e in entries.flatten() {
        let path = e.path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".dump") {
            continue;
        }
        count += 1;
        let bytes = std::fs::read(&path).unwrap();
        if let Err(err) = rvcore::run_isa_test(&bytes, 50_000_000) {
            failures.push(format!("{name}: {err}"));
        }
    }
    assert!(failures.is_empty(), "{} of {count} failed:\n{}", failures.len(), failures.join("\n"));
    eprintln!("{count} riscv-tests passed");
}
