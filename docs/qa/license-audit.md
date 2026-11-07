# Third-party license audit — v1

Owner: QA (audit), foundation pair (packaging). Gate for E11-T09. This file records what ships, under which license, and how the notices are produced.

## 1. Policy

- Every Rust dependency must be under a permissive license compatible with `MIT OR Apache-2.0`: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, or a documented dual license.
- GPL, AGPL, LGPL (static), SSPL, Commons Clause, and non-commercial licenses are rejected unless a written exception is recorded here.
- Every bundled sample must have a recorded license and attribution: CC0, original work, or a written license. No sample of unknown provenance ships.
- The portable ZIP and the installer both carry `THIRD-PARTY-NOTICES.txt` generated from the resolved dependency graph, not from a hand-written list.
- The audit runs against `Cargo.lock`, not `Cargo.toml`, so transitive crates are covered.

## 2. Generate the inventory

```powershell
cargo install cargo-about --locked   # once per machine
cargo about generate about.hbs -o THIRD-PARTY-NOTICES.txt
cargo metadata --locked --format-version 1 | jq '.packages[] | {name, version, license}' > docs/qa/license-inventory.json
```

`cargo deny` is the CI gate:

```powershell
cargo install cargo-deny --locked
cargo deny check licenses
```

`deny.toml` must allow only the licenses in §1 and deny everything else.

## 3. Inventory

The resolved graph does not exist yet in this checkout (`Cargo.lock` predates the current crate dependencies). Run §2 when the workspace builds and paste the generated table here. Do not hand-maintain rows for transitive crates.

| Crate | Version | License | Use | Notices |
| --- | --- | --- | --- | --- |
| (generated — see `license-inventory.json`) | | | | |

Direct dependencies expected at v1 (confirm from the lock file): `serde`, `serde_json`, `thiserror`, `uuid`, `sha2`, `hex`, `zip`, `hound`, `jsonschema`, `symphonia`, `rubato`, `log`, `tracing`, `tracing-subscriber`, `tracing-appender`, `directories`, `cpal`, `midir`, `wasapi`, `winit`, `egui`, `eframe`, `egui_kittest`, `criterion`, `tempfile`.

Flag on sight: `symphonia` codec features (MP3/AAC patent notes), `zip` (bzip2/zstd optional features), any crate with a `LICENSE-*` split, and any crate whose build script downloads a binary.

## 4. Samples and assets

| Asset | Source | License | Attribution required | Recorded in |
| --- | --- | --- | --- | --- |
| Factory demo kit | | CC0 or original | | |
| App icon | | | | |
| Any bundled impulse/noise | | | | |

Rule: a kit is not added to the repo until its row exists. The factory demo kit bead (E05-T08) owns filling this table; QA blocks the release if it is empty.

## 5. Notices file

`THIRD-PARTY-NOTICES.txt` must:

- list every crate from the generated inventory with version and license text reference;
- include the full text of each distinct license;
- include the sample attribution table;
- be shipped in both the portable ZIP and the installer;
- be regenerated as part of the release build, and fail the build if it differs from the committed copy.

## 6. Installer and signing

- Installer: per-user, no admin, uninstall entry, no services or startup entries.
- Signing: optional for the first release; if a certificate is available, sign both the installer and the executable and record the thumbprint and timestamp authority here.
- SmartScreen: document that unsigned artifacts may warn, publish the SHA-256, and link the verification command:

  ```powershell
  Get-FileHash .\drumpad-<version>-windows-x64.zip -Algorithm SHA256
  ```

## 7. Sign-off

| Check | Status |
| --- | --- |
| `cargo deny check licenses` passes | pending |
| Inventory generated from `Cargo.lock` and committed | pending |
| Notices file present in ZIP and installer | pending |
| Sample licenses recorded | pending |
| SmartScreen/hash guidance in release notes | pending |
