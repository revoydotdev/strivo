# SPA language boundary

The embedded daemon SPA remains an ordered, concatenated vanilla-JavaScript
bundle. Its globals and Rust build-time module filtering make a wholesale
`.js` → `.ts` conversion unnecessarily risky. The marketing surface and
licence worker remain TypeScript with their existing strict checks.

New SPA work should introduce typed boundaries incrementally: keep Rust
`serde` types authoritative, add JSDoc models (and `allowJs`/`checkJs`) around
API clients or isolated new modules, and preserve the existing PVR/Creator
assembly and bundle checks. Do not make the Rust build depend on an ambient
Node compiler until an explicit generated-JavaScript pipeline is agreed.
