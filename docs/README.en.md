# cang-jie

**[中文](../README.md)**

A device-enhancement suite for the reMarkable Paper Pro Move — built **without modifying
`xochitl`** (the device's stock reading app) itself. Everything runs as xovi extensions plus a
set of standalone web services alongside the official system.

## What this is

A personal-use toolkit built around one e-ink tablet: get books onto the device, optionally
clean up/convert/optimize them, choose which reader to hand them to (the stock `xochitl` reader
or KOReader); highlight text and jot handwritten notes next to it, have those automatically
picked up, transcribed and answered by an AI model on your phone, then projected back into the
device's own notebooks or into Obsidian. On top of that, a handful of low-level single-purpose
enhancements: precise CJK highlight-snapping, handwritten-stroke rendering tuning, a battery
diagnostics sampler, and one-upload font/wallpaper installs. Everything runs on top of the
device's stock firmware via [xovi](https://github.com/asivery/xovi) (a third-party extension
loader) plus a set of lightweight, independent web services — no repackaging or patching of
`xochitl` itself.

## What's in here

Six independent top-level project lines, each individually installable:

| Project | What it is |
|---|---|
| [`shelf/`](../shelf/) — bookshelf | Import books → clean up / convert / optimize → deliver to the stock library or KOReader; a web UI plus a host-side CLI |
| [`notes/`](../notes/) — notes pipeline | Highlighted text + handwritten notes next to it, auto-ingested on closing the book → review/transcribe/ask-AI on your phone → project back into device notebooks or Obsidian |
| [`enhance/`](../enhance/) — device enhancements | Precise CJK highlight-snap and handwritten-stroke rendering tuning (two standalone xovi extensions); a battery diagnostics sampler; upload-and-use font/wallpaper web services |
| [`gateway/`](../gateway/) — web gateway | The single shared web entry point for the three lines above: HTTPS (private CA) + login password + reverse proxy to each domain service |
| [`rmsvc-core/`](../rmsvc-core/) — service foundation | Shared infrastructure crate for the web services above (paths / service registry / HTTP adapter / event bus, etc.) — no business logic |
| [`packaging/`](../packaging/) — installer | One command to install everything above on a fresh device (with firmware-compatibility checking) |

> No standalone docs inside each directory — the feature descriptions above are all there is; browse the code itself for detail.

## Recent updates

Only actual new features/capabilities, not a full commit log.

| Date | Added |
|---|---|
| 2026-09-14 | Scanned comic PDFs (common from sources like Anna's Archive) are now correctly recognized as comics and routed through that pipeline, instead of failing outright on push |
| 2026-09-14 | Re-running a batch push now actually skips the processing step for books that already succeeded, not just the upload — saves computer time, not just bandwidth |
| 2026-09-14 | Fixed a "device time sync often shows as failed right after a reboot" issue: the root cause was autosuspend interrupting the very first time-sync attempt after boot; the actual time was already correct, only the status display was wrong |
| 2026-09-13 | Batch book transfers are now duplicate-safe: both the web upload and the desktop command-line push check beforehand whether a file was already transferred, so a partial failure followed by an as-is rerun won't re-transfer a book that already succeeded into a duplicate |
| 2026-09-13 | A Sidebar shortcut straight to "KOReader"; if the third-party WeRead app is installed it's auto-detected and a "WeRead" entry appears too — both are wired into `packaging/`, repeatable via `install-all.sh` |
| 2026-09-13 | The gateway's "Setup · foundation" page now also probes whether WeRead is installed (read-only, same as the KOReader check) |

## Quick start

Only supports the **reMarkable Paper Pro Move on firmware 3.28.0.172** (the only version
verified so far).

```sh
git clone https://github.com/bbq191/rm-tweak.git
cd rm-tweak/packaging
sh install-all.sh 10.11.99.1
```

For full prerequisites, a step-by-step breakdown, the firmware safety gate, and
troubleshooting, see **[INSTALL.en.md](INSTALL.en.md)**.

## History and scope

This project started out as a "reMarkable Chinese input method" (its internal development
codename `cang-jie` / 仓颉 refers to the mythical inventor of Chinese characters — this page's
title keeps that name, but the public repository is published under the more fitting
[`rm-tweak`](https://github.com/bbq191/rm-tweak)), then grew reading-enhancement,
personal-knowledge-management, and handwriting-recognition directions over time. On
2026-09-11 the repository went through a large-scale cleanup: the Chinese input method, the
full reading pipeline, and the PKM/handwriting-recognition lines — along with their reverse-
engineering groundwork — were moved out of the internal development repository entirely (not
deleted, just relocated to a directory on the maintainer's own machine that isn't tracked by
git). **Those features are still deployed and running on the device today**; their source just
isn't distributed with this public codebase, and they're no longer under active development.
What's actively
maintained in this repository today is the six lines listed above, built with a more
conservative architecture (standalone web services plus a deliberately small xovi-extension
surface) than the earlier, more deeply invasive hook suite.

## Sponsor

If this project has been useful to you, feel free to buy the author a coffee — entirely
optional, and has no effect on any feature whether you do or don't. QR codes and how the funds
are used are on a separate page: **[DONATE.en.md](DONATE.en.md)**.

## License / disclaimer

The code in this repository is open-sourced under the **[Apache License 2.0](../LICENSE)** — free
to use, modify, and distribute (commercial use included), provided attribution is kept; the
license includes an express patent grant. Originally built as a personal tool; provided as-is,
with no warranty of any kind.

Where third-party licensing is relevant (dictionary data, fonts, etc., if any are pulled in),
the in-code comments record what was actually verified at the time — this is not legal advice.
Not affiliated with reMarkable, [xovi](https://github.com/asivery/xovi), vellum, KOReader, or
any other third-party project or trademark mentioned here; each remains the property of its
respective owner.
