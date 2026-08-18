# Research UI modules (Creator Edition only)

These ES modules build the Coding Studio surfaces over the research kernel
(`crates/research`). They are imported from `spa.js` inside a
`/* @creator-start */ … /* @creator-end */` block, and `build.rs` deletes this
whole directory from the PVR build — so nothing here ships in the free edition.

One file per owner so the surfaces can be developed in parallel without
colliding in the 13k-line `spa.js`:

| File | Surface |
|---|---|
| `codebook.js` | Code tree (create/edit), codings list, apply-coding |
| `corpus.js`   | Sources browser, cases, signal browser |
| `notebook.js` | Memos, relationships, coder agreement, export |

Each module exports `mount(root, ctx)` where `root` is the container element
and `ctx` carries `{ api, projectId, toast, fmt }` from the SPA shell. Modules
must not reach into `spa.js` internals beyond `ctx`.
