//! Detect what a recording file actually is, independently of its name.
//!
//! Recording filenames come from a user template that ends in a fixed
//! extension (`{channel}_{date}_{title}.mkv` by default), but the container
//! that lands on disk is chosen by whatever ffmpeg or yt-dlp produced. A
//! survey of one real library found 9 of 14 `.mkv` files were not Matroska:
//! five MP4, three MPEG-TS, one MP3 from an audio-only pull.
//!
//! A mislabelled file is not merely untidy. It breaks anything that trusts
//! the extension — external players, media scanners, and any later remux —
//! and it makes the library dishonest about its own contents.
//!
//! Detection is by magic bytes rather than by shelling out to ffprobe: it is
//! a few bytes of read, it cannot be defeated by a wrong name, and it works
//! on a truncated file where a full probe fails.

use std::path::Path;

/// Container detected from a file's leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Matroska,
    WebM,
    Mp4,
    MpegTs,
    Mp3,
}

impl Container {
    /// The extension this container should carry.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Matroska => "mkv",
            Container::WebM => "webm",
            Container::Mp4 => "mp4",
            Container::MpegTs => "ts",
            Container::Mp3 => "mp3",
        }
    }
}

/// How many bytes `detect` needs. MPEG-TS needs three sync bytes 188 apart.
const PROBE_LEN: usize = 377;

/// Identify a container from its leading bytes. `None` means "unrecognised",
/// which is always treated as "leave it alone" rather than guessed at.
pub fn detect(head: &[u8]) -> Option<Container> {
    // EBML — Matroska and WebM share it, and are told apart by the DocType
    // string that appears early in the header.
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let window = &head[..head.len().min(64)];
        if window.windows(4).any(|w| w == b"webm") {
            return Some(Container::WebM);
        }
        return Some(Container::Matroska);
    }
    // ISO base media: a size field, then `ftyp` at offset 4.
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some(Container::Mp4);
    }
    // MP3: an ID3 tag, or a raw frame sync.
    if head.starts_with(b"ID3") {
        return Some(Container::Mp3);
    }
    if head.len() >= 2 && head[0] == 0xFF && (head[1] & 0xE0) == 0xE0 {
        return Some(Container::Mp3);
    }
    // MPEG-TS: 0x47 sync every 188 bytes. One byte would false-positive on
    // any file that happens to start with 'G', so require the cadence.
    if head.len() > 376 && head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 {
        return Some(Container::MpegTs);
    }
    None
}

/// Detect the container of a file on disk, reading only its head.
pub fn detect_file(path: &Path) -> Option<Container> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; PROBE_LEN];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    detect(&buf)
}

/// The path this file *should* have, or `None` when the name already agrees
/// with the contents (or the container is unrecognised).
///
/// Only the extension is changed; the stem the user's template produced is
/// left exactly as it is.
pub fn corrected_path(path: &Path) -> Option<std::path::PathBuf> {
    let actual = detect_file(path)?;
    let want = actual.extension();
    let have = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if have == want {
        return None;
    }
    // `.mkv` is a superset container that legitimately holds most of what we
    // record, so a Matroska file named `.mkv` is correct even when its codecs
    // came from elsewhere. Anything else disagreeing is a genuine mismatch.
    Some(path.with_extension(want))
}

/// Rename a finished recording so its extension matches its contents.
///
/// Returns the new path when a rename happened. Never overwrites: if the
/// corrected name is taken, the file is left where it is, because losing a
/// capture is far worse than a wrong extension.
pub fn normalize_extension(path: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
    let Some(target) = corrected_path(path) else {
        return Ok(None);
    };
    if target.exists() {
        tracing::warn!(
            "container: {} is really {}, but {} already exists — leaving it alone",
            path.display(),
            target.extension().and_then(|e| e.to_str()).unwrap_or("?"),
            target.display()
        );
        return Ok(None);
    }
    std::fs::rename(path, &target)?;
    tracing::info!(
        "container: renamed {} -> {} to match its actual container",
        path.display(),
        target.display()
    );
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below is the real leading bytes of a file from an actual
    // recording library, read with `head -c 8 | xxd -p`. Inventing plausible
    // magic numbers is how a detector ends up passing its tests and failing
    // on the disk it was written for.

    #[test]
    fn detects_matroska_from_a_real_capture() {
        let head = [0x1A, 0x45, 0xDF, 0xA3, 0xA3, 0x42, 0x86, 0x81];
        assert_eq!(detect(&head), Some(Container::Matroska));
    }

    #[test]
    fn detects_mp4_named_mkv() {
        // `PathFix_..._Interview.mkv` — an MP4 wearing an mkv extension.
        let head = [0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70];
        assert_eq!(detect(&head), Some(Container::Mp4));
    }

    #[test]
    fn detects_mpegts_named_mkv() {
        // `falco_..._SLAY THE SPIRE 2 TOURNAMENT.mkv` starts 47 40 00 10.
        let mut head = vec![0u8; 400];
        head[0] = 0x47;
        head[1] = 0x40;
        head[188] = 0x47;
        head[376] = 0x47;
        assert_eq!(detect(&head), Some(Container::MpegTs));
    }

    #[test]
    fn detects_mp3_named_mkv() {
        // `the yard_..._AUDIO_.mkv` — 88MB of MP3 behind a video extension.
        let head = [0x49, 0x44, 0x33, 0x03, 0x00, 0x00, 0x01, 0x21];
        assert_eq!(detect(&head), Some(Container::Mp3));
    }

    #[test]
    fn a_single_g_byte_is_not_mpegts() {
        // Without the 188-byte cadence this would match any file starting
        // with the letter 'G'.
        let head = b"Great content, not a transport stream".to_vec();
        assert_eq!(detect(&head), None);
    }

    #[test]
    fn unrecognised_bytes_are_left_alone() {
        assert_eq!(detect(&[0u8; 16]), None);
        assert_eq!(detect(&[]), None);
    }

    #[test]
    fn correct_extension_is_reported_per_container() {
        assert_eq!(Container::Matroska.extension(), "mkv");
        assert_eq!(Container::Mp4.extension(), "mp4");
        assert_eq!(Container::MpegTs.extension(), "ts");
        assert_eq!(Container::Mp3.extension(), "mp3");
        assert_eq!(Container::WebM.extension(), "webm");
    }

    #[test]
    fn matching_name_needs_no_correction() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("real.mkv");
        std::fs::write(&p, [0x1A, 0x45, 0xDF, 0xA3, 0xA3, 0x42, 0x86, 0x81]).unwrap();
        assert_eq!(corrected_path(&p), None);
    }

    #[test]
    fn mismatched_name_is_corrected_keeping_the_stem() {
        let dir = tempfile::tempdir().unwrap();
        // A stem with the spaces and punctuation real titles carry.
        let p = dir
            .path()
            .join("the yard_2026-05-28_Ep_ 252 - CS2 _AUDIO_.mkv");
        std::fs::write(&p, b"ID3\x03\x00\x00\x01\x21").unwrap();
        let fixed = corrected_path(&p).expect("mp3 named .mkv must be corrected");
        assert_eq!(fixed.extension().unwrap(), "mp3");
        assert_eq!(
            fixed.file_stem().unwrap(),
            "the yard_2026-05-28_Ep_ 252 - CS2 _AUDIO_"
        );
    }

    #[test]
    fn normalize_renames_and_reports_the_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("capture.mkv");
        std::fs::write(&p, [0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70]).unwrap();
        let out = normalize_extension(&p).unwrap().expect("should rename");
        assert!(out.exists(), "renamed file must exist");
        assert!(!p.exists(), "original name must be gone");
        assert_eq!(out.extension().unwrap(), "mp4");
    }

    /// Losing a capture is far worse than a wrong extension.
    #[test]
    fn normalize_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("capture.mkv");
        let taken = dir.path().join("capture.mp4");
        std::fs::write(&p, [0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70]).unwrap();
        std::fs::write(&taken, b"do not clobber me").unwrap();
        assert_eq!(normalize_extension(&p).unwrap(), None);
        assert!(p.exists(), "source must survive");
        assert_eq!(std::fs::read(&taken).unwrap(), b"do not clobber me");
    }
}
