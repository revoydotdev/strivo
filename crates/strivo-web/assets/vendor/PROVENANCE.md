# Vendored browser libraries

Three pre-built third-party browser libraries, vendored verbatim. They are the
whole of the "add two pre-built libraries rather than adopt a framework" decision
in the plan's `Must NOT have` section: there is no bundler, no minifier change
and no Node toolchain in the release path, and nothing here is transformed on
the way in. Each file is byte-identical to the artefact its upstream package
publishes, and `tests/vendor_provenance.rs` fails the build if that stops being
true.

## Why these are vendored at all

`crates/strivo-web/build.rs` embeds the whole `assets/` tree into the binary and
serves it from `/assets/{*path}`. Serving a file is the cheapest possible
delivery mechanism, and it is the only one that adds no build step: a vendored
`.js`/`.mjs` file is copied into `$OUT_DIR/assets`, embedded by `rust-embed`, and
reached by a `import()` at the point of use. A bundler would be a new dependency
and a new failure mode in exchange for nothing these three do not already do.

## Files

| File | Library | Version | Upstream artefact | Licence |
|---|---|---|---|---|
| `mediabunny.min.mjs` | [mediabunny](https://www.npmjs.com/package/mediabunny) | 1.60.0 | `dist/bundles/mediabunny.min.mjs` | MPL-2.0 |
| `x6.umd.js` | [@antv/x6](https://www.npmjs.com/package/@antv/x6) | 3.1.8 | `dist/x6.min.js` | MIT |
| `wavesurfer.esm.js` | [wavesurfer.js](https://www.npmjs.com/package/wavesurfer.js) | 8.0.1 | `dist/wavesurfer.esm.js` | BSD-3-Clause |

Each package's upstream licence text is vendored next to it as
`LICENSE.mediabunny-MPL-2.0.txt`, `LICENSE.antv-x6-MIT.txt` and
`LICENSE.wavesurfer-BSD-3-Clause.txt`. Mediabunny's licence was not stated
anywhere in the work plan and was read from the published package: `1.60.0`
declares `MPL-2.0` in its `package.json`, ships the full MPL-2.0 text as
`LICENSE`, and the file's own banner reproduces the MPL-2.0 grant.

`x6.umd.js` is the only filename here that is not the upstream one. `@antv/x6`
publishes `dist/x6.min.js`; it is renamed to state the *module form* rather than
the minification, because the form is the load-bearing property (see below) and
because the plan's import path is `/assets/vendor/x6.umd.js`.

## SHA-256

Re-run `sha256sum` in this directory to verify, or let
`cargo test -p strivo-web --test vendor_provenance` do it and fail loudly on a
mismatch.

| File | SHA-256 | Bytes |
|---|---|---:|
| `mediabunny.min.mjs` | `ea3f1a537e64aa99e1e5af7d066977d4fac90bc6d71617200b4ab8147fb17f5a` | 684 785 |
| `x6.umd.js` | `0cda942c20b32e3e2b3b8ff11560aefda39008a8c00d43c75295e4a5543593b0` | 583 499 |
| `wavesurfer.esm.js` | `079f3b142f37737f975b03ea8fd18652a316dc2120a2629aeca0844a0c08f2b1` | 51 732 |

## How each is loaded, and why

All three load through a **dynamic `import()`**, never a static one. `spa.js` is
a single concatenated module, so a static top-level import of 684 785 bytes of
Mediabunny would put all of it in the initial parse path and make the
compressed-size budget in `docs/PERFORMANCE.md` unreachable by construction.

| File | Form | Loaded as |
|---|---|---|
| `mediabunny.min.mjs` | self-contained minified ESM, 100+ named exports | `const { … } = await import("/assets/vendor/mediabunny.min.mjs")` |
| `x6.umd.js` | UMD, no ESM exports | `await import("/assets/vendor/x6.umd.js")`; read `globalThis.X6` |
| `wavesurfer.esm.js` | self-contained ESM, default export only | `const { default: WaveSurfer } = await import("/assets/vendor/wavesurfer.esm.js")` |

**`.mjs` must be served as JavaScript.** `mime_for()` in
`src/routes/assets.rs` had four cases and served everything else as
`application/octet-stream`; a browser refuses to execute that as an ES module, so
a vendored `.mjs` was fetched and then silently refused — indistinguishable in
the browser from a 404. The extension now has a case, and
`src/routes/assets.rs`'s unit test and `tests/routes.rs` both assert the served
`Content-Type`.

### X6: why UMD is viable here

`@antv/x6` has no browser-ready ESM bundle. Its `es/` tree is 337 unbundled
modules, and assembling it would mean adding the bundler this plan forbids, so
`dist/x6.min.js` (UMD) is what gets vendored. A UMD bundle has no ESM exports, so
`import { Graph } from …` is a link-time `SyntaxError` that would take down the
whole concatenated `spa.js`; a dynamic side-effect import is legal, and the
library is then read off `globalThis.X6`.

That is only safe if the UMD wrapper prefers `globalThis`, because at ES-module
top level `this` is `undefined` and a `})(this, …)` wrapper would throw a
`TypeError` on `root.X6 = …`. The shipped bundle's first bytes are:

```js
!function(t,e){"object"==typeof exports&&"undefined"!=typeof module?e(exports):"function"==typeof define&&define.amd?define(["exports"],e):e((t="undefined"!=typeof globalThis?globalThis:t||self).X6={})}(this,function(t){"use strict";
```

The `(this, …)` invocation is the common form, but the factory **reassigns** `t`
with `"undefined"!=typeof globalThis ? globalThis : (t || self)` before attaching,
so `globalThis` wins and the import attaches correctly. This was verified by
reading those bytes and by evaluating the import in a real browser, not assumed.

## Edition boundary

These are Creator-only dependencies. `build.rs` deletes `assets/vendor/` from the
embedded tree when the `creator` feature is off and asserts the directory is
gone, so no PVR binary embeds or serves these bytes; `tests/routes.rs` proves
`GET /assets/vendor/…` is 404 from a PVR build, and the same file asserts the
source tree really does contain all three — a boundary test whose subject is
empty would pass for free.
