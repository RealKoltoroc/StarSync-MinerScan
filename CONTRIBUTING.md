# Contributing

Contributions are welcome.

## Development baseline

- Windows 10/11 x64
- stable Rust / MSVC toolchain
- current tested baseline: Rust 1.98.0

Before submitting a change:

```powershell
cargo check
cargo test
cargo build --release
```

Please keep changes focused, preserve the local-only OCR/game-client boundary, and avoid introducing telemetry or game-process injection.

## Data changes

Do not hard-code provider responses as authoritative live values. Static game-data baselines should be versioned and attributed. Dynamic market/refinery information should remain provider-backed/cached and clearly distinguish estimates from verified values.

## Privacy

Do not commit personal paths, usernames, API keys, logs, screenshots containing account information, or `%LOCALAPPDATA%\StarSync MinerScan` runtime data.

## Licensing

By contributing original code, you agree that it can be distributed under GPL-3.0-or-later. Do not submit third-party code/assets/data unless their redistribution terms are understood and documented in `THIRD_PARTY_NOTICES.md`.
