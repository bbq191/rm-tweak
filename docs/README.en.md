# cang-jie

**[中文](../README.md)**

A device-enhancement suite for the reMarkable Paper Pro Move. It **does not modify `xochitl`** (the device's stock
reading/notes app). Instead it adds features with two things: small plugins loaded by [xovi](https://github.com/asivery/xovi)
(a third-party extension loader), and a set of web services that run on the device. What it adds: book management,
note organizing, and some small system-level improvements.

## What it does

| You want | What provides it | Where |
|---|---|---|
| 2026-09-25 | When the on-device trash / new-folder agent fails 5 times, a banner at the top of the web page tells you (dismiss with "Got it"); adding handwriting beside a highlight, or erasing it, keeps the same note entry with its proofread status and text |
| 2026-09-25 | **Fourth full-system audit** (50+ fixes): note entries come back when you undo an erase or restore a book from the trash, keeping proofread text; a half-read `.metadata` is no longer taken as "book deleted"; oversized comics split for delivery no longer lose the cover and front pages; a book still being copied in by scp is no longer ingested half-written; `&` in titles and tables of contents no longer shows up as `&amp;`; services no longer stop silently on transient connection errors; a corrupt `.rm` or PDF no longer crashes a service. Power: the font service no longer rescans every font at boot, the on-device UI agents back off on errors (15→120 s), and upload staging moved from the RAM disk to /home. The installer makes fewer ssh round trips and reports a failed reboot honestly. This round was tested on the development machine only (314 simulated install checks), not yet on a device |
| Get books onto the device and make them read well on e-ink | Upload on the web page or fetch an article; books land in the "master library" first. EPUBs get one-click "optimize" (layout, table of contents, cover, footnotes); PDFs with a text layer are converted to EPUB keeping their original formatting. Then deliver to the stock reader or KOReader (KOReader comes with two reading presets, one for text books and one for comics). Books over xochitl's ~100MB upload limit work too; comics get dedicated handling; books in the master library can be downloaded as the original file, renamed, or given a per-book reading direction | [`shelf/`](../shelf/) |
| Turn highlighter marks and handwritten notes beside them into organizable notes | Collected automatically when you close the book. On your phone: review the handwriting transcription, full-text search, ask an AI model; then send back into a device notebook (headings, lists and checkboxes use the device's own styles) or export as Obsidian markdown | [`notes/`](../notes/) |
| Small system-level improvements | Precise CJK highlighter snapping, handwriting stroke-width tuning, tap-to-turn and a manga (right-to-left) page-turn rule in the xochitl reader, battery drain diagnostics, upload-and-use fonts and wallpapers | [`enhance/`](../enhance/) |
| One place on your phone/computer to operate all of the above | A single HTTPS web entry with a login password that forwards requests to each service, plus a device-health page (service status, which extensions are loaded, boot timing, last lines of the previous boot's log) | [`gateway/`](../gateway/) |
| Install everything on a new device with one command | With a firmware-compatibility check, and a one-command uninstall; the device is checked automatically after deploying | [`packaging/`](../packaging/) |

One "behind the scenes" directory: [`rmsvc-core/`](../rmsvc-core/) (the shared base library of the web services, no business logic).

Everything runs on the device's **stock system**; `xochitl` is never repackaged.

## Recent updates

| Date | Added |
|---|---|
| 2026-09-25 | **Changes now take effect through a full device reboot** (back in about 20–60 s) instead of restarting xochitl alone: stopping xochitl turned out to crash on exit now and then (its own shutdown-order bug, unrelated to the extensions), which then triggers an emergency reboot anyway; a clean, deliberate reboot is predictable. The installer waits for the device to come back and runs the read-only `packaging/verify-on-device.sh` check (44 checks in 9 groups). A manual reboot also swaps in pending extension updates |
| 2026-09-25 | New "Device health" page under Manage (xochitl/xovi status, loaded extensions, per-service restarts and memory peak, boot timing, last log lines of the previous boot, cleanup of leftover data); a banner after a firmware update says a reinstall is needed; "Move to xochitl trash" is now carried out by a resident on-device agent within seconds |
| 2026-09-25 | Per-book reading direction in the master library (auto / right-to-left / left-to-right, batch-capable), written into the book and synced to the manga page-turn rule; note markers `##` / `###` / `1.` / `-` / `- [ ]` map one-to-one to the device's built-in text styles, and generated notes carry the book's section names; wider distance for pairing handwriting with a highlight |
| 2026-09-24 | Third full-system audit: several battery savings (create-folder long poll 25 → 290 s, wallpaper service now watches file reads, LAN-name service wakes only on IP changes), large books no longer push the notes service over its memory limit, read-idle timeouts on every service connection, and the private CA is now name-constrained (**after upgrading, reinstall the certificate on every phone and computer**, see [INSTALL.en.md](INSTALL.en.md#after-installing)) |
| 2026-09-24 | "Tap to turn" and the manga page-turn rule for the xochitl reader (Manage → System enhancements, off by default); KOReader text / comic presets; full-text search across notes; download original and rename in the master library; the original PDF is kept for 7 days after PDF→EPUB conversion; 9 background services no longer wait for the network at boot |
| 2026-09-23 | PDFs with a text layer are converted to EPUB keeping their formatting (colors, image position and aspect, links); several EPUB-optimizer fixes and two new quality checks; an "Import KOReader annotations" button on the notes page; the battery diagnostic's resident process no longer forks |
| 2026-09-22 | Comic/PDF split-delivery now streams for real, fixing a real-device OOM risk on large collections (one case measured 233MB → 93MB peak); the image-downsampling safety threshold was re-calibrated from real-device measurements — the previous formula-estimated value was too low and the safeguard was essentially a no-op |
| 2026-09-22 | Install/uninstall scripts overhauled: pre-flight checks (ssh reachability, firmware allowlist, free device space), on-demand restart (xochitl no longer restarts every run if nothing actually changed), `--dry-run` to preview without touching the device, and uninstall now runs in reverse order and cleans up its own staged payload directories |
| 2026-09-22 | The appload 3.28 compatibility patch tooling is fully retired — upstream appload v0.6.0 now natively supports 3.28/3.29, so it's just `vellum upgrade appload` |
| 2026-09-22 | Master-library page polish: the "add to library" folder picker now reads real device folders; the create-folder web proxy switched to long-polling |
| 2026-09-22 | The notes pipeline now also ingests KOReader highlights/vocabulary, running alongside the existing xochitl native-reader annotation pipeline |

## Quick start

Only the **reMarkable Paper Pro Move on firmware 3.28.0.172** is supported. It is the only version verified so far; the installer checks and refuses otherwise by default.

1. Install four base components on the device by hand: `vellum add xovi`, `vellum add qt-resource-rebuilder`, `vellum add appload` (≥ 0.6.0), then sideload KOReader through appload. They are not part of this project and the installer won't install them.
2. Your computer needs a Rust cross-compilation setup and passwordless ssh to the device's root (list in [INSTALL.en.md "Before you install"](INSTALL.en.md#before-you-install)). Connect the device over USB, then:

   ```sh
   git clone https://github.com/bbq191/rm-tweak.git
   cd rm-tweak/packaging
   sh install-all.sh --dry-run        # rehearse first: prints the plan, never touches the device
   sh install-all.sh 10.11.99.1
   ```
   The last step reboots the device once (about a minute); when it is back, the script checks it and marks each item ✓/⚠/✗.
3. Open `https://10.11.99.1/` in a browser. The default password is `shelf`, and you must change it on first login.

To uninstall: `sh uninstall-all.sh 10.11.99.1`. Prerequisites, risks and how to recover after a firmware update (OTA) are all in **[INSTALL.en.md](INSTALL.en.md)**.

## History and scope

The project started as a "reMarkable Chinese input method" (its internal development codename `cang-jie` / 仓颉
refers to the mythical inventor of Chinese characters — this page's title keeps that name, but the public repository
is published under the more fitting [`rm-tweak`](https://github.com/bbq191/rm-tweak)), then grew
reading-enhancement, personal-knowledge-management and handwriting-recognition directions. On 2026-09-11 the
repository went through a large cleanup: the Chinese input method, the full reading pipeline and the
PKM/handwriting-recognition lines — along with their reverse-engineering groundwork — were moved out of the internal
development repository entirely (not deleted, just relocated to a directory on the maintainer's own machine that
isn't tracked by git). **Those features are still deployed and running on the device today**; their source just
isn't distributed with this public codebase, and they're no longer under active development. What's actively
maintained in this repository today is the lines listed above, built with a more conservative architecture
(standalone web services plus a deliberately small xovi-extension surface) than the earlier, more deeply invasive
hook suite.

## Sponsor

If this project has been useful to you, feel free to buy the author a coffee — entirely optional, and it has no effect on any feature. QR codes etc. are on **[DONATE.en.md](DONATE.en.md)**.

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
