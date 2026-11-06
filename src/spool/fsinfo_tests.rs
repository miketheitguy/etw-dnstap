//! Tests for the spool folder checks that need a junction or a drive code.

use std::{fs, path::Path, process::Command};

use super::*;
use crate::spool::test_support::TestDir;

/// Makes a junction at `link` that points to `target` with `mklink /J`, which
/// needs no elevation, so a failure is a failure of the test.
fn make_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .expect("mklink /J starts");
    assert!(
        output.status.success(),
        "mklink /J exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_junction_as_the_spool_folder_is_refused() {
    let dir = TestDir::new("fsinfo-junction");
    let target = dir.path().join("target");
    fs::create_dir(&target).expect("target folder");
    let link = dir.path().join("link");
    make_junction(&link, &target);
    let result = check_spool_dir(&link);
    fs::remove_dir(&link).expect("junction is removed");
    assert!(
        matches!(
            result,
            Err(SpoolDirError::ReparsePoint {
                which: "spool folder"
            })
        ),
        "{result:?}"
    );
}

#[test]
fn a_junction_as_the_parent_of_the_spool_folder_is_refused() {
    let dir = TestDir::new("fsinfo-junction-parent");
    let target = dir.path().join("target");
    fs::create_dir_all(target.join("child")).expect("target and child folders");
    let link = dir.path().join("link");
    make_junction(&link, &target);
    let result = check_spool_dir(&link.join("child"));
    fs::remove_dir(&link).expect("junction is removed");
    assert!(
        matches!(
            result,
            Err(SpoolDirError::ReparsePoint {
                which: "parent of the spool folder"
            })
        ),
        "{result:?}"
    );
}
