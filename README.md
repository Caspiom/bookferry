<div align="center">

# Bookferry

**A local-first book & comic reader for desktop and Android, with reading-position sync over your own network — no cloud, no third-party servers.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange.svg)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/Caspiom/bookferry)](https://github.com/Caspiom/bookferry/releases)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20Android-informational)](https://bookferry.gaspari.dev)

[**Website & downloads →**](https://bookferry.gaspari.dev)

</div>

---

## What is Bookferry?

Bookferry reads **EPUB, PDF and CBZ** on your computer and your phone, and keeps your **reading position in sync** between them — so you close a book on your PC and pick it up on the couch exactly where you stopped.

The twist: there is **no cloud and no account**. Your computer *is* the server. A lightweight HTTPS server (written in Rust) runs embedded inside the desktop app; your phone pairs with it over the local Wi-Fi and syncs directly, device-to-device. Your library never leaves your machines.

## Highlights

- 📚 **Three formats, one reader** — EPUB (via `foliate-js`), PDF (via `pdf.js` over HTTP Range) and CBZ comics (double-page spreads, RTL, fit-to-page/width/height, double-tap zoom).
- 🔄 **Reading-position sync** — resume exactly where you left off across devices, with conflict-free merging (HLC timestamps + last-writer-wins).
- 🏠 **Local-first & private** — the desktop app embeds the server; the phone is a pure client. No VPN, no SaaS, no telemetry. Files are downloaded on demand from your own PC.
- 🔒 **Secure by design** — TLS with certificate pinning (`rustls` + `rcgen`), device pairing via a one-time secret → token, and single-use pairing payloads.
- 📡 **Zero-config discovery** — the phone finds your PC on the LAN automatically via mDNS (`_bookferry._tcp`).
- 🖥️ **Truly cross-platform** — one Rust core, shipping to Linux, Windows, macOS and Android from a single codebase (Tauri v2).
- 🎨 **Comfortable reading** — per-book typography presets, immersive mode, volume-key page turns on Android, custom covers and a "Continue reading" shelf.

## Architecture

```
                 same Wi-Fi (TLS + cert pinning, mDNS discovery)
   ┌────────────────────────┐                      ┌────────────────────────┐
   │     Desktop app        │   position sync      │     Android app        │
   │  (Tauri v2 + WebView)  │ ◀──── HTTPS/SSE ────▶ │  (Tauri v2 + WebView)  │
   │                        │                      │                        │
   │  ┌──────────────────┐  │                      │  ┌──────────────────┐  │
   │  │ bookferry-server │  │                      │  │ bookferry-client │  │
   │  │  (axum + rustls) │  │                      │  │   (hyper + TLS)  │  │
   │  └────────┬─────────┘  │                      │  └──────────────────┘  │
   │  ┌────────┴─────────┐  │                      │                        │
   │  │ bookferry-client │  │  (loopback pairing)  │                        │
   │  └──────────────────┘  │                      │                        │
   │   SQLite · your files  │                      │   SQLite cache         │
   └────────────────────────┘                      └────────────────────────┘
```

- **`bookferry-core`** — shared domain: `BookId` (partial-MD5 + size, KOReader-compatible), Hybrid Logical Clocks, last-writer-wins merge, pairing payloads.
- **`bookferry-server`** — embeddable HTTPS server: `axum` + `tokio-rustls`, `rusqlite` for state, `rcgen` for self-signed certs, `mdns-sd` for LAN advertisement, Server-Sent Events for live position updates, HTTP Range for streaming large files (a 592 MB CBZ opens in ~22 ms, reading only the ZIP central directory).
- **`bookferry-client`** — networking client: persistent outbox, HLC persistence, SSE with automatic reconnection and pull, resume-decision logic (use-local / use-remote / ask).
- **`app/`** — the Tauri v2 application (shared JS frontend + per-platform Rust): the desktop embeds server + client; Android ships only the client.

## Tech stack

**Rust** (workspace of 3 crates + the Tauri backend) · `axum` · `tokio` · `rustls` / `tokio-rustls` · `rcgen` · `rusqlite` (bundled SQLite) · `mdns-sd` · `serde`
**App:** Tauri v2 · `foliate-js` · `pdf.js` · Kotlin (Android bridges) · **Nix** (reproducible dev environment)

## Repository layout

```
crates/
  bookferry-core/     # shared types, HLC, LWW, BookId
  bookferry-server/   # embeddable HTTPS + sync server
  bookferry-client/   # sync client (outbox, SSE, resume logic)
app/                  # Tauri v2 app (desktop + Android) — see app/README.md
docs/                 # design docs, spike reports, MVP notes
nixos/                # Nix packaging & dev module
```

## Build & run

The project uses a reproducible **Nix** dev shell. In short:

```bash
# Desktop (Linux)
cd spike && nix develop -c bash -c 'cd .. && cargo run -p bookferry-app'

# Full test suite: format + clippy (warnings as errors) + workspace tests
cd spike && nix develop -c bash -c 'cd .. && cargo fmt --check \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo test --workspace'
```

Full desktop/Android/APK build instructions live in **[`app/README.md`](app/README.md)**.
Prebuilt installers (`.deb`, `.rpm`, `.AppImage`, `.exe`, `.dmg`, `.apk`) are on the
[**releases page**](https://github.com/Caspiom/bookferry/releases) and at
[**bookferry.gaspari.dev**](https://bookferry.gaspari.dev).

## Status

Early but working — current release **v0.1.1**. Reading (EPUB/PDF/CBZ), LAN pairing,
position sync, custom covers and the "Continue reading" shelf are implemented and
tested end-to-end on Linux desktop and a real Android device. An optional stateless
relay (for syncing while away from home) is planned for a later release. Installers
are not yet code-signed, so Windows SmartScreen / macOS Gatekeeper may warn on first run.

## License

Released under the [MIT License](LICENSE). © 2026 Lucas Gaspari.
