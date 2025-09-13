// tools/dump_sources.rs
// Combine all .rs files in the project (except this file) into a single text file.
// Additionally include ".env", "config.yaml", and "Cargo.toml".
// Each section starts with the relative path header like: "===== ./src/main.rs =====".
//
// Run with:
//   cargo run --bin dump_sources
// or specify output path:
//   cargo run --bin dump_sources -- ./data/rs_snapshot.txt

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

// Files to always include (in addition to all *.rs)
const EXTRA_FILENAMES: &[&str] = &[".env", "config.yaml", "Cargo.toml"];

fn main() -> std::io::Result<()> {
    let root = PathBuf::from(MANIFEST_DIR);

    // Output path: default "<project>/all_rs_sources.txt" or from argv[1]
    let args: Vec<String> = std::env::args().collect();
    let out_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        root.join("all_rs_sources.txt")
    };

    // Resolve this source file path to exclude it from results
    let self_src = root.join("tools").join("dump_sources.rs");
    let self_canon = self_src.canonicalize().ok();

    let mut files = Vec::new();
    collect_files(&root, &mut files)?;

    // Keep only .rs OR extra filenames, and exclude this tool itself
    files.retain(|p| should_include_file(p));
    if let Some(self_canon) = self_canon {
        files.retain(|p| p.canonicalize().map(|c| c != self_canon).unwrap_or(true));
    }

    // Sort for deterministic output by relative path
    files.sort_by(|a, b| {
        let ar = a.strip_prefix(&root).unwrap_or(a);
        let br = b.strip_prefix(&root).unwrap_or(b);
        ar.cmp(br)
    });

    // Write output
    if let Some(parent) = out_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut out = File::create(&out_path)?;
    for path in files {
        let rel = path.strip_prefix(&root).unwrap_or(&path);
        writeln!(out, "===== ./{} =====", rel.display())?;

        let mut content = String::new();
        File::open(&path)?.read_to_string(&mut content)?;
        out.write_all(content.as_bytes())?;
        if !content.ends_with('\n') {
            writeln!(out)?;
        }
        writeln!(out)?; // blank line between files
    }

    println!("Wrote {}", out_path.display());
    Ok(())
}

/// Recursively collect all files under `dir`, skipping common noise folders.
fn collect_files(dir: &Path, acc: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if is_ignored_dir(&path) {
                continue;
            }
            collect_files(&path, acc)?;
        } else {
            acc.push(path);
        }
    }
    Ok(())
}

/// Decide whether a file should be included in the dump.
fn should_include_file(p: &Path) -> bool {
    // include all .rs files
    if p.extension().and_then(|e| e.to_str()) == Some("rs") {
        return true;
    }
    // include specific extra files by exact filename, anywhere in the tree
    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
        return EXTRA_FILENAMES.contains(&name);
    }
    false
}

fn is_ignored_dir(p: &Path) -> bool {
    if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
        matches!(name, "target" | ".git" | ".idea" | ".vscode")
    } else {
        false
    }
}
