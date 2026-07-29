//! Build script for strivo-web.
//!
//! In PVR mode (no `creator` feature), strips `/* @creator-start */` …
//! `/* @creator-end */` blocks from `spa.js` and writes the cleaned copy
//! to `$OUT_DIR/assets/`.  Creator mode copies assets unchanged.
//!
//! `src/assets.rs` points `RustEmbed` at `$OUT_DIR/assets` so it always
//! picks up the (possibly-stripped) file rather than the source tree.

use std::{env, fs, io, path::Path};

fn main() {
    // Rerun when any source asset changes or the feature set changes.
    println!("cargo:rerun-if-changed=assets/spa.js");
    println!("cargo:rerun-if-changed=assets/spa.css");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_CREATOR");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let creator_enabled = env::var("CARGO_FEATURE_CREATOR").is_ok();

    let src_assets = Path::new(&manifest_dir).join("assets");
    let dst_assets = Path::new(&out_dir).join("assets");

    copy_dir_all(&src_assets, &dst_assets).expect("failed to copy assets to OUT_DIR");
    // The old 1246px PNG is retained in source for packaging/brand exports,
    // but the SPA now uses the 220-byte SVG mark. Do not embed nearly 1 MiB
    // of unreachable raster data into every web binary.
    let _ = fs::remove_file(dst_assets.join("img/chorosyne-logo.png"));

    if !creator_enabled {
        let spa_src = src_assets.join("spa.js");
        let spa_dst = dst_assets.join("spa.js");
        let content = fs::read_to_string(&spa_src).expect("failed to read spa.js");
        let stripped = strip_creator_blocks(&content);
        fs::write(&spa_dst, stripped).expect("failed to write stripped spa.js");
    }
}

/// Copy a directory tree from `src` to `dst`, creating `dst` if needed.
fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path)?;
        }
    }
    Ok(())
}

/// Remove every line from `/* @creator-start */` through `/* @creator-end */`
/// (inclusive).  The surrounding non-creator lines are kept verbatim.
///
/// The markers live inside a JS object literal where each removed block ends
/// with a comma on the last kept property, so stripping lines never leaves a
/// trailing-comma or missing-comma syntax error.
fn strip_creator_blocks(content: &str) -> String {
    let mut out: Vec<&str> = Vec::with_capacity(content.lines().count());
    let mut in_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "/* @creator-start */" {
            in_block = true;
            continue;
        }
        if trimmed == "/* @creator-end */" {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push(line);
        }
    }
    let mut result = out.join("\n");
    result.push('\n');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_blocks_and_preserves_rest() {
        let src = "a,\n  /* @creator-start */\n  b,\n  c,\n  /* @creator-end */\n  d,\n";
        let out = strip_creator_blocks(src);
        assert!(!out.contains("b,"), "creator line must be removed");
        assert!(!out.contains("c,"), "creator line must be removed");
        assert!(out.contains("a,"), "non-creator line must be kept");
        assert!(out.contains("d,"), "non-creator line must be kept");
        assert!(!out.contains("@creator-start"), "marker must be removed");
        assert!(!out.contains("@creator-end"), "marker must be removed");
    }

    #[test]
    fn strip_handles_multiple_blocks() {
        let src = "x,\n  /* @creator-start */\n  y,\n  /* @creator-end */\n  z,\n  /* @creator-start */\n  w,\n  /* @creator-end */\n  end,\n";
        let out = strip_creator_blocks(src);
        assert!(out.contains("x,"));
        assert!(out.contains("z,"));
        assert!(out.contains("end,"));
        assert!(!out.contains("y,"));
        assert!(!out.contains("w,"));
    }
}
