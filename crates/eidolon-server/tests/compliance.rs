//! Automated governance and compliance test gate for the eidolon workspace.
//!
//! Enforces:
//! 1. Absolute zero third-party runtime dependencies.
//! 2. Agent stealth boundaries and gitignore isolation.
//! 3. Zero em dash (Unicode U+2014) invariant across all repository files.
//! 4. Compiler safety invariants (`unsafe_code = "deny"`).

use std::fs;
use std::path::{Path, PathBuf};

fn get_workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // manifest_dir is crates/eidolon-server -> parent is crates -> parent is workspace root
    manifest_dir
        .parent()
        .expect("crates directory")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn test_zero_third_party_dependencies() {
    let root = get_workspace_root();
    let cargo_paths = [
        root.join("Cargo.toml"),
        root.join("crates/eidolon-core/Cargo.toml"),
        root.join("crates/eidolon-net/Cargo.toml"),
        root.join("crates/eidolon-spatial/Cargo.toml"),
        root.join("crates/eidolon-world/Cargo.toml"),
        root.join("crates/eidolon-server/Cargo.toml"),
    ];

    for path in &cargo_paths {
        assert!(path.exists(), "Cargo manifest must exist: {:?}", path);
        let content = fs::read_to_string(path).expect("Read Cargo.toml");

        // Parse lines in [dependencies] or [workspace.dependencies]
        let mut in_deps_section = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_deps_section =
                    trimmed == "[dependencies]" || trimmed == "[workspace.dependencies]";
                continue;
            }

            if in_deps_section && !trimmed.is_empty() && !trimmed.starts_with('#') {
                // Must be a workspace reference or path dependency
                let is_internal_dependency =
                    trimmed.contains("path =") || trimmed.contains("workspace = true");
                assert!(
                    is_internal_dependency,
                    "Violation of Section 2.2 (Zero Third-Party Dependencies) in {:?}: '{}'",
                    path, trimmed
                );
            }
        }
    }
}

#[test]
fn test_gitignore_agent_stealth_boundaries() {
    let root = get_workspace_root();
    let gitignore_path = root.join(".gitignore");
    assert!(gitignore_path.exists(), ".gitignore must exist at root");

    let content = fs::read_to_string(gitignore_path).expect("Read .gitignore");
    assert!(
        content.contains("AGENTS.md"),
        ".gitignore must explicitly shield AGENTS.md"
    );
    assert!(
        content.contains(".agents/"),
        ".gitignore must explicitly shield .agents/ directory"
    );
    assert!(
        content.contains("scratch/"),
        ".gitignore must explicitly shield scratch/ directory"
    );
}

#[test]
fn test_zero_em_dash_invariant() {
    let root = get_workspace_root();
    let mut files_to_scan = Vec::new();

    fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();

                if name == "target" || name == ".git" {
                    continue;
                }

                if path.is_dir() {
                    collect_files(&path, files);
                } else {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext == "rs"
                        || ext == "md"
                        || ext == "toml"
                        || ext == "json"
                        || ext == "yml"
                        || ext == "yaml"
                    {
                        files.push(path);
                    }
                }
            }
        }
    }

    collect_files(&root, &mut files_to_scan);
    assert!(
        !files_to_scan.is_empty(),
        "Must have files to scan for em dashes"
    );

    let mut violations = Vec::new();
    for path in files_to_scan {
        if let Ok(content) = fs::read_to_string(&path) {
            if content.contains('\u{2014}') {
                violations.push(path);
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Violation of Section 2.10 (Zero Em Dash Rule). Found Unicode U+2014 in: {:?}",
        violations
    );
}

#[test]
fn test_workspace_compiler_safety_lints() {
    let root = get_workspace_root();
    let root_cargo = fs::read_to_string(root.join("Cargo.toml")).expect("Read root Cargo.toml");

    assert!(
        root_cargo.contains("unsafe_code = \"deny\""),
        "Workspace must deny unsafe code by default"
    );
    assert!(
        root_cargo.contains("all = \"deny\""),
        "Workspace must deny Clippy warnings"
    );
}
