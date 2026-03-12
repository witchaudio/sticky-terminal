# StickyTerminal

StickyTerminal is a desktop app that combines:

- a real terminal on the right
- markdown notes on the left

The goal is to make it easy to work in tools like Codex CLI while keeping notes, todos, and project thoughts in the same window.

## What it does

- runs a live shell inside the app
- supports multiple terminal tabs
- lets you drag to reorder tabs
- supports tab rename and close from the context menu
- lets you drag files or folders into the terminal to paste their paths
- gives you a collapsible notes sidebar
- saves notes into a `StickyTerminal` folder you choose
- renders markdown previews in the notes panel
- supports privacy mode for screen sharing on macOS
- starts the shell as a login shell on macOS so commands like `codex` use your normal PATH
- includes custom app icons for runtime and macOS app bundles

## Built with

- Rust
- `eframe` / `egui`
- `portable-pty`
- `vt100`
- `pulldown-cmark`
- `rfd`

## Run locally

Make sure Rust is installed, then run:

```bash
cargo run
```

To check that the project builds:

```bash
cargo check
```

## Build a real macOS app

This repo now includes a local build script that creates a normal app bundle:

```bash
./scripts/build-macos-app.sh
```

That will build:

```bash
dist/StickyTerminal.app
```

You can then drag `StickyTerminal.app` into your Applications folder.

Each time you want an updated app:

1. pull or make your latest code changes
2. run `./scripts/build-macos-app.sh`
3. replace the old app in Applications with the new `dist/StickyTerminal.app`

If you launch the app from Finder, the terminal will start in your home folder by default.

## Build a shareable DMG

To make a nice installer DMG with a drag-to-Applications layout:

```bash
./scripts/build-dmg.sh
```

That will build:

```bash
dist/StickyTerminal-0.1.0.dmg
```

## Sign and notarize for sharing

If you have the paid Apple Developer Program, you can sign and notarize the app properly.

### 1. Install your Developer ID certificate

You need this certificate in your login keychain:

```text
Developer ID Application
```

You can create and download it from your Apple Developer account, then install it by opening the downloaded certificate file on your Mac.

To check if it is installed:

```bash
security find-identity -v -p codesigning
```

### 2. Save your notary credentials

Create a Keychain profile for notarization:

```bash
./scripts/setup-notary-profile.sh stickyterminal-notary your-apple-id@example.com YOURTEAMID
```

Apple will prompt for an app-specific password if needed.

### 3. Sign and notarize everything

Then run:

```bash
./scripts/sign-and-notarize.sh
```

This script will:

1. build the app
2. sign it with your Developer ID certificate
3. notarize the app
4. staple the app ticket
5. build the DMG
6. sign the DMG
7. notarize the DMG
8. staple the DMG ticket

## Notes

When you open the notes panel:

1. choose a folder
2. StickyTerminal will use a `StickyTerminal` folder there
3. open or create a markdown note
4. write in one raw markdown note view

## Current themes

- Terminal
- Black
- Blue
- Red

## macOS icon assets

The repo includes:

- runtime app icon pngs in `assets/`
- macOS bundle icon files in `assets/macos/`

## Status

This is an active personal project and the UI is still evolving.
