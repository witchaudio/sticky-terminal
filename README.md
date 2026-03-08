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
- gives you a collapsible notes sidebar
- saves notes into a `StickyTerminal` folder you choose
- renders markdown previews in the notes panel
- supports privacy mode for screen sharing on macOS
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

## Notes

When you open the notes panel:

1. choose a folder
2. StickyTerminal will use a `StickyTerminal` folder there
3. open or create a markdown note
4. write in `Write` mode or switch to `Preview`

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
