# windows-tools

Personal tools for Windows 11: a family of native taskbar widgets written in Rust, plus a few
helpers for working with local LLMs, Codex and PDFs.

## Taskbar widgets

Small native apps (Rust, Win32 + GDI, no web view, ~600 KB each) that dock into the Windows 11
taskbar next to the tray. They stack together, can be reordered by dragging, open a panel on
hover or click, and follow Windows settings live (light/dark theme, animation effects, text size,
display scale).

| Folder | What it shows |
|---|---|
| [`taskbar-dock`](taskbar-dock) | The shared library every widget runs on: docking and stacking, panels, right-click menu, tray icon, settings, theme and drawing. Start here to build a new widget ([BLOCK.md](taskbar-dock/BLOCK.md)). |
| [`taskbar-weather`](taskbar-weather) | Current weather and forecast ([Open-Meteo](https://open-meteo.com/)). |
| [`taskbar-media`](taskbar-media) | Now playing (Spotify, browsers, any Windows media session): cover, progress ring, playback buttons. |
| [`taskbar-discord`](taskbar-discord) | Your Discord voice call: who's in it and who's speaking (Discord's local RPC). |
| [`buswidget`](buswidget) | Next bus departures from a stop, live times plus a GTFS timetable (Dutch public transport, ovapi.nl). |
| [`taskbar-sysmon`](taskbar-sysmon) | CPU / GPU / RAM / network monitor. Planned, not built yet: [plan](taskbar-sysmon/PLAN.md) and [mockup](taskbar-sysmon/mockup.html). |

All widgets share one settings file, `%APPDATA%\TaskbarDock\settings.json`, with a section per
widget; "Edit settings…" in any widget's right-click menu opens it.

### Build

Needs Rust with the MSVC toolchain. In a widget's folder (`buswidget\rust` for the bus widget):

```bash
cargo build --release
```

The exe ends up in `target\release\`. Keep the checkout at a short path: MSVC `link.exe` fails on
paths over 260 characters.

## Other tools

| Folder | What it does |
|---|---|
| [`local-llm`](local-llm) | An MCP server that lets Claude Code hand bulk work (summarizing, extraction, classification, transcription, search) to a local model via Ollama. |
| [`pdf2md`](pdf2md) | PDF → Markdown with math (Marker), with automatic QA against the PDF text layer so only problem spots need review. |
| [`codex`](codex) | A PowerShell wrapper for calling OpenAI Codex from Claude Code for builds and reviews. |

## Notes

These are personal tools, built for one machine: some paths, settings and defaults (the bus
stop, the GPU, conda env names) are specific to it. Each folder's README has the details.

## License

[MIT](LICENSE)
