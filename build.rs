//! Takes the version from the nearest git tag (vX.Y.Z, set by CI on every merge
//! to main), so `ttycast --version` names the exact build. Outside a git
//! checkout (a crate tarball) it falls back to the version in Cargo.toml.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    let described = Command::new("git")
        .args(["describe", "--tags", "--match", "v[0-9]*.[0-9]*.[0-9]*"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
    let version = match described {
        Some(tag) => tag.trim().trim_start_matches('v').to_string(),
        None => std::env::var("CARGO_PKG_VERSION").unwrap_or_default(),
    };
    println!("cargo:rustc-env=TTYCAST_VERSION={version}");
}
