// The single fixture accessor (plan todo 32).
//
// Every lane that needs a piece of representative media resolves it here
// rather than hard-coding a path: the browser E2E lane today, and todos 30,
// 43, 44, 45, 48 and 67 when they land. `MANIFEST.json` is the
// cross-language contract — a Rust test reads the same file with serde_json —
// so this module is the JS-side convenience over it, not a second source of
// truth.
//
// Nothing here probes or decodes. Proving what a fixture *is* is
// `scripts/verify_media_fixtures.py`'s job, which runs ffprobe and ffmpeg. A
// test that wants a property should read `fixture(name).ffprobe` — the
// recorded reading — or shell out; it should never infer a class from the
// file name.
import { createHash } from "node:crypto";
import { readFileSync, existsSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const CORPUS_DIR = dirname(fileURLToPath(import.meta.url));
export const MANIFEST_PATH = join(CORPUS_DIR, "MANIFEST.json");

export function loadManifest() {
  return JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
}

/** Absolute paths of every fixture, in manifest order. */
export function fixturePaths() {
  return loadManifest().fixtures.map((f) => join(CORPUS_DIR, f.name));
}

/**
 * One fixture's record, with `path` resolved. Throws when the name is not in
 * the manifest, so a typo fails loudly instead of resolving to nothing.
 */
export function fixture(name) {
  const entry = loadManifest().fixtures.find((f) => f.name === name);
  if (!entry) {
    const known = loadManifest().fixtures.map((f) => f.name).join(", ");
    throw new Error(`no fixture named ${JSON.stringify(name)}; have: ${known}`);
  }
  return { ...entry, path: join(CORPUS_DIR, entry.name) };
}

export function byClass(klass) {
  return loadManifest().fixtures.filter((f) => f.class === klass);
}

export function classes() {
  return [...new Set(loadManifest().fixtures.map((f) => f.class))].sort();
}

export function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/**
 * Re-check every fixture against the manifest. Returns one row per fixture so
 * a caller can assert on the array rather than on a boolean — a check whose
 * subject is empty passes, which is the failure mode this returns rows to
 * prevent.
 */
export function integrity() {
  return loadManifest().fixtures.map((entry) => {
    const path = join(CORPUS_DIR, entry.name);
    if (!existsSync(path)) return { name: entry.name, ok: false, detail: "missing" };
    const size = statSync(path).size;
    if (size !== entry.bytes) {
      return { name: entry.name, ok: false, detail: `size ${size} != ${entry.bytes}` };
    }
    const digest = sha256(path);
    if (digest !== entry.sha256) {
      return { name: entry.name, ok: false, detail: `sha256 ${digest} != ${entry.sha256}` };
    }
    return { name: entry.name, ok: true, detail: `${size} B` };
  });
}
