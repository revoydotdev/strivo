//! Build script for strivo-web.
//!
//! `assets/spa.js` and `assets/spa.css` are each assembled at build time from
//! ordered module files (under `assets/spa/` and `assets/spa-css/`
//! respectively) rather than shipped as one file in source. Each module is
//! named `<seq>-<edition>.{js,css}` where `<edition>` is `pvr` (always
//! included) or `creator` (included only when the `creator` Cargo feature is
//! on); the assembler concatenates them in filename order into a single
//! `$OUT_DIR/assets/spa.{js,css}`, which is what `src/assets.rs`'s
//! `RustEmbed` actually serves.
//!
//! This mirrors the pattern `assets/research/` already used: a PVR build is
//! produced by *including* PVR sources, never by deleting lines out of a
//! shared file. `assets/research/` (the Creator-only research UI) is still
//! dropped wholesale for PVR builds, exactly as before — it has no PVR
//! content to keep.
//!
//! `src/assets.rs` points `RustEmbed` at `$OUT_DIR/assets` so it always picks
//! up the assembled tree rather than the source tree.

use std::{env, fs, io, path::Path};

fn main() {
    // Watch the whole asset tree: naming individual files here meant a new
    // module could be edited without triggering a rebuild, so the binary would
    // silently keep serving the previous copy.
    println!("cargo:rerun-if-changed=assets");
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

    assemble_modules(&dst_assets, "spa", "js", creator_enabled).expect("failed to assemble spa.js");
    assemble_modules(&dst_assets, "spa-css", "css", creator_enabled)
        .expect("failed to assemble spa.css");

    if !creator_enabled {
        // The research UI is Creator-only in its entirety; ship none of it.
        let _ = fs::remove_dir_all(dst_assets.join("research"));
        // Same for the vendored browser libraries, which only Creator surfaces
        // import. `let _ =` swallows the error, so the assert below is the gate:
        // an embedded asset is a binary-size cost *and* a fetchable path.
        let vendor = dst_assets.join("vendor");
        let _ = fs::remove_dir_all(&vendor);
        assert!(
            !vendor.exists(),
            "PVR build still embeds {}",
            vendor.display()
        );
    }

    // Release builds ship a minified bundle. Beyond the size win, this is
    // what removes source comments from the served asset: the SPA is
    // otherwise shipped verbatim, so explanatory comments naming Creator
    // plugins survive into the binary and show up under `strings` even
    // though no Creator code or UI is present (ADR 0002's invisibility
    // bar). Debug builds keep the readable bundle for development.
    if env::var("PROFILE").as_deref() == Ok("release") {
        minify_js(&dst_assets.join("spa.js")).expect("failed to minify spa.js");
        minify_css(&dst_assets.join("spa.css")).expect("failed to minify spa.css");
        // strivo.css is a small static sheet served alongside the SPA.
        let strivo_css = dst_assets.join("strivo.css");
        if strivo_css.exists() {
            minify_css(&strivo_css).expect("failed to minify strivo.css");
        }
        if creator_enabled {
            // The research UI ships as separate ES modules, not through the
            // assembled bundle, so it needs minifying in its own right.
            minify_js_tree(&dst_assets.join("research"))
                .expect("failed to minify research modules");
        }
    }
}

/// Minify one JavaScript file in place.
///
/// Deliberately parse-and-regenerate only: whitespace and comments are
/// dropped, but identifiers are NOT mangled and the compressor is NOT run.
/// The bundle is hand-written, relies on top-level function hoisting across
/// concatenated modules, and guards Creator entry points with
/// `typeof f === "function"` — all of which the aggressive passes in this
/// version of the minifier are not proven safe against. Comment and
/// whitespace removal is what the invisibility bar actually needs, and it
/// is the transform with a behaviour-preserving guarantee we can state.
fn minify_js(path: &Path) -> io::Result<()> {
    use oxc_allocator::Allocator;
    use oxc_codegen::{CodeGenerator, CodegenOptions};
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    let source = fs::read_to_string(path)?;
    let allocator = Allocator::default();
    // The SPA is served as `<script type="module">`.
    let source_type = SourceType::js().with_module(true);
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    assert!(
        parsed.errors.is_empty(),
        "refusing to minify {}: parse reported {} error(s); first: {:?}",
        path.display(),
        parsed.errors.len(),
        parsed.errors.first()
    );
    let out = CodeGenerator::new()
        .with_options(CodegenOptions {
            single_quote: false,
            minify: true,
        })
        .build(&parsed.program)
        .source_text;
    fs::write(path, out)
}

/// Minify every `.js` file under `dir`, recursively. No-op if absent.
fn minify_js_tree(dir: &Path) -> io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            minify_js_tree(&path)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("js") {
            minify_js(&path)?;
        }
    }
    Ok(())
}

/// Minify one stylesheet in place.
fn minify_css(path: &Path) -> io::Result<()> {
    use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};

    let source = fs::read_to_string(path)?;
    let mut sheet = StyleSheet::parse(&source, ParserOptions::default())
        .unwrap_or_else(|e| panic!("refusing to minify {}: {e}", path.display()));
    sheet
        .minify(MinifyOptions::default())
        .unwrap_or_else(|e| panic!("failed to minify {}: {e}", path.display()));
    let out = sheet
        .to_css(PrinterOptions {
            minify: true,
            ..Default::default()
        })
        .unwrap_or_else(|e| panic!("failed to print {}: {e}", path.display()));
    fs::write(path, out.code)
}

/// Build `$OUT_DIR/assets/spa.<ext>` by concatenating the ordered module
/// files under `assets/<module_dir>/`, in filename order, then delete the
/// source module directory from the embedded tree so it is never itself
/// served (a PVR build must not expose `assets/<module_dir>/*-creator.<ext>`
/// as a fetchable path).
///
/// Filtering happens by *including* the modules a build wants, not by
/// deleting lines from a shared file — a PVR build simply never reads the
/// `-creator.<ext>` files. Used for both `spa.js` (`assets/spa/`) and
/// `spa.css` (`assets/spa-css/`).
fn assemble_modules(
    dst_assets: &Path,
    module_dir: &str,
    ext: &str,
    creator_enabled: bool,
) -> io::Result<()> {
    let src_dir = dst_assets.join(module_dir);
    let mut modules: Vec<_> = fs::read_dir(&src_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    modules.sort();

    let creator_suffix = format!("-creator.{ext}");
    let mut assembled = String::new();
    for module in &modules {
        let name = module.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_creator = name.ends_with(&creator_suffix);
        if is_creator && !creator_enabled {
            continue;
        }
        assembled.push_str(&fs::read_to_string(module)?);
    }

    fs::write(dst_assets.join(format!("spa.{ext}")), assembled)?;
    fs::remove_dir_all(&src_dir)?;
    Ok(())
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
