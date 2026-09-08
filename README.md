# StarSync MinerScan

StarSync MinerScan is a lightweight Windows companion tool for Star Citizen mining and scanning workflows. It continuously captures a user-defined screen region, performs local OCR on scanner signature values, resolves recognized values against a local target database, and can trigger sound, logging, enrichment and self-contained HTML reports.

<img width="920" height="1210" alt="image" src="https://github.com/user-attachments/assets/c98dffbe-238e-46a8-bc6e-536f242ff0de" />



## Highlights

- Rust / eframe-egui native desktop application.
- Windows DXGI Desktop Duplication capture of only the configured OCR region, with GDI fallback.
- Local OCR using `ocrs` + RTen models embedded in the executable.
- Passive global Numpad hotkeys that do not reserve the keys from the foreground game.
- Configurable monitor and calibration ROI.
- Mineral, salvage and asteroid signature matching, including cluster multipliers.
- Duplicate suppression with a configurable same-material cooldown.
- Optional LIVE OCR preview in a fixed bottom-panel tab.
- Persistent event log and enriched self-contained HTML reports.
- Local-first refinery calculator with configurable method efficiency assumptions.
- SCUnpacked update inbox under `%LOCALAPPDATA%\StarSync MinerScan\database`.
- Optional cached enrichment from UEX, Star Citizen Wiki API and StarCitizen.Tools.
- System-tray operation.

## Download

The current Windows build is available in `release/StarSyncMinerScan_0.6.8.exe`.

No installer is required. Start the executable from any writable directory. On first start it creates:

```text
%LOCALAPPDATA%\StarSync MinerScan\
  settings.json
  minerscan-events.log
  cache\
  session-reports\
  database\
```

## SCUnpacked updates

The executable contains a baseline dataset so it works immediately. To update supported game-data records, place a current SCUnpacked Data ZIP or extracted dataset in:

```text
%LOCALAPPDATA%\StarSync MinerScan\database
```

Then use **Settings > Data Sources > INSTALL/UPDATE**. MinerScan can refresh commodity/mineral records and discover newly added relevant locations/refineries from the newer dataset. Provider-specific refinery modifiers can still be supplied from the optional UEX cache/update layer.

## Privacy and game-client boundary

MinerScan is a screen-reading companion utility. It does **not** inject into Star Citizen, read game memory, automate game input, intercept game network traffic or modify the Star Citizen process. OCR is local. External network requests occur only for providers explicitly enabled in Settings. See `docs/PRIVACY.md`.

## Build from source

See `docs/RUST_BUILD.md`. The tested development baseline is Rust 1.98.0 / Cargo 1.98.0 on `x86_64-pc-windows-msvc`.

Quick build:

```powershell
cargo test
cargo build --release
```

## Credits and data providers

StarSync MinerScan benefits from the work of several Star Citizen community projects:

- [SCUnpacked Data](https://github.com/StarCitizenWiki/scunpacked-data) — unpacked game-data snapshots used for local metadata/update workflows.
- [UEX](https://uexcorp.space/) / [UEX API](https://uexcorp.space/api/documentation) — optional community market/refinery enrichment.
- [Star Citizen Wiki API](https://api.star-citizen.wiki/) / [source](https://github.com/StarCitizenWiki/API) — optional structured game-data and media enrichment.
- [StarCitizen.Tools](https://starcitizen.tools/) — optional wiki text and media enrichment.
- [ocrs](https://github.com/robertknight/ocrs) and [Ocrs pretrained models](https://huggingface.co/robertknight/ocrs) — local OCR stack.

See `THIRD_PARTY_NOTICES.md` for licensing and attribution details.

## Support

The software is free and is intended to remain free. If MinerScan is useful to you and you want to support continued maintenance, testing and future Star Citizen compatibility work, voluntary support is welcome via Patreon:

There are no paid feature locks implied by this support link.

## License

The original StarSync MinerScan source code in this repository is licensed under **GNU GPL-3.0-or-later**. Third-party libraries, OCR models, community data and game-derived content retain their own licenses/terms and are not relicensed by this project. See `LICENSE` and `THIRD_PARTY_NOTICES.md`.

## Disclaimer

This is an unofficial fan-made project and is not affiliated with, endorsed by, or sponsored by Cloud Imperium Games or Roberts Space Industries. Star Citizen and related names, trademarks and game content belong to their respective owners.
