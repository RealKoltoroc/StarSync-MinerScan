# Third-Party Notices

StarSync MinerScan includes or interacts with third-party libraries, model artifacts, community datasets and services. The GPL license for the original MinerScan source code does **not** override the terms that apply to these materials.

## Ocrs / RTen OCR

- Project: https://github.com/robertknight/ocrs
- Pretrained models: https://huggingface.co/robertknight/ocrs
- Model training repository: https://github.com/robertknight/ocrs-models

The `ocrs` software is distributed under MIT OR Apache-2.0. The official Hugging Face repository for the pretrained Ocrs models identifies the model repository as **CC BY-SA 4.0**. MinerScan embeds the text detection and text recognition RTen model files to keep OCR local and make the portable executable self-contained.

Model attribution: Ocrs pretrained text detection and recognition models by Robert Knight and contributors.

## SCUnpacked Data

- Data repository: https://github.com/StarCitizenWiki/scunpacked-data
- Related loader lineage: https://github.com/StarCitizenWiki/scunpacked

MinerScan contains a curated/versioned baseline derived from community-produced unpacked Star Citizen game data and supports importing newer SCUnpacked Data snapshots. The `scunpacked-data` repository is public but does not currently present a clear standalone data license in its repository metadata. Game-derived records also remain subject to the rights of their respective owners. This project does not claim ownership over Star Citizen game data.

## UEX

- Website: https://uexcorp.space/
- API documentation: https://uexcorp.space/api/documentation

UEX is an optional community-data provider for current market/refinery information. UEX data is community-maintained and may differ from live game values. MinerScan caches provider responses locally when the provider is enabled.

## Star Citizen Wiki API

- API: https://api.star-citizen.wiki/
- Source: https://github.com/StarCitizenWiki/API

Used optionally for structured Star Citizen game data and media references. The API project requests credit in public projects. Its source repository is MIT licensed; data returned by the service can include third-party/game-derived material with separate rights.

## StarCitizen.Tools

- Website: https://starcitizen.tools/

Used optionally for wiki text and media enrichment. Wiki content is generally published under Creative Commons Attribution-ShareAlike unless otherwise noted on the relevant page/file.

## Rust dependencies

The Rust crates listed in `Cargo.toml` and their transitive dependencies retain their individual upstream licenses. `Cargo.lock` is included to make the dependency graph reproducible.

## Cloud Imperium Games / Star Citizen

This project is unofficial and is not affiliated with, endorsed by, or sponsored by Cloud Imperium Games or Roberts Space Industries. Star Citizen, Roberts Space Industries, Cloud Imperium and related marks/content are the property of their respective owners. Game-derived data is used for informational fan-tool functionality only.
