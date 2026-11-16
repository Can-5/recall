# Recall

**A local, searchable, capped history engine for Orbit — with explicit exclusions.**

> Orbit companion module — phase 2/4 of the [Orbit ecosystem](https://github.com/Can-5/orbit).
> Standalone crate: usable without Orbit, without other modules, and without network.

## Implemented now

- HistoryEntry model (text / image / link) with favorites and timestamps
- RecallHistory: bounded ring with favorites-sticky eviction; full-text substring search
- ExclusionRules: plain-text denylist (source apps and content) — checked before recording

## Not implemented yet (explicitly out of scope for this crate)

- No screen scraping or OCR capture — that is the Orbit platform layer (ScreenshotCapture permission)
- No UI, no indexing daemon, no sync

## Build & test

```sh
cargo test  # 6 tests
```

Dependencies: serde, serde_json, thiserror. No cross-repo path dependencies, no Git dependencies, no downloads at build time beyond crates.io.

## Privacy

Exclusions are enforced by contract: excluded content is never recorded, even transiently. Nothing leaves the device; there is no network code.

## License

MIT © The Orbit contributors. See [LICENSE](./LICENSE).
