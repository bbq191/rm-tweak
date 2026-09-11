# Installation Guide

**[中文](INSTALL.md)** · back to [README](README.en.md)

## Scope

**reMarkable Paper Pro Move (imx93-chiappa), firmware 3.28.0.172** — the only firmware version
verified on real hardware so far; the installer checks this automatically before doing anything
(see "Firmware safety gate" below). Other firmware versions or other reMarkable models are
unverified — forcing an install there risks misaligned QML injection offsets (best case, a
feature silently doesn't work; worst case, it affects normal device operation).

## Before you install: a few things you need to do by hand

These are reMarkable's own / third-party ecosystem infrastructure, not part of this
repository — `install-all.sh` will **not** install them for you. If one is missing, the
relevant step fails with a clear message telling you what to run:

1. **`vellum add xovi`** — the [xovi](https://github.com/asivery/xovi) extension loader itself.
   Most of what this repository does runs as xovi extensions, so this is the most basic
   prerequisite.
2. **`vellum add qt-resource-rebuilder`** — QML hot-resource replacement. A few optional
   features in `shelf`/`gateway` (the font menu, the trash/new-folder web proxy) depend on it;
   without it, those two features are silently skipped and nothing else is affected.
3. **`vellum add appload`** — the third-party app loader.
4. Sideload **KOReader** through appload.

For how to install vellum itself, or the specifics of sideloading appload/KOReader, refer to
vellum's and the reMarkable community's own documentation — this is general device
infrastructure that isn't maintained by this project, so it isn't repeated here.

**Optional, not a prerequisite**: **WeRead** (a third-party reMarkable WeChat Reading app) —
not something this repository can install; you'd download the official release and install it
over SSH yourself. The Sidebar-entry step under "Install everything with one command" below
auto-detects whether this device has it installed: if so, the Sidebar shortcut becomes the
"KOReader + WeRead" two-entry version; if not, you just get "KOReader" — it won't error out or
skip the whole step just because WeRead isn't there.

## Recommended install order

Doing it in this order minimizes rework (the numbered risk items referenced below are in "Risk
warnings" further down):

1. **Check the firmware version first** — under Settings, only 3.28.0.172 is verified so far
   (see "Scope" above).
2. **Install the ecosystem infrastructure by hand** (previous section), in dependency order —
   don't skip a step:
   1. `vellum add xovi`
   2. `vellum add qt-resource-rebuilder`
   3. `vellum add appload` — once installed, confirm the native "AppLoad" icon actually shows
      up in the sidebar (⚠ risk ①: the official release may not work correctly on 3.28; fix
      that first, or the Sidebar-entry step later will silently skip)
   4. Sideload KOReader through appload
   5. (optional) Want the WeRead sidebar shortcut? Install and log into WeRead first, following
      its own official release instructions
3. **Run `install-all.sh`** (no `--skip`, install everything at once):
   ```sh
   cd packaging && sh install-all.sh 10.11.99.1
   ```
4. **Check the closing summary** — confirm the "installed" list matches what you expected, and
   nothing was silently skipped that you actually wanted (skipped ≠ failed, easy to miss — see
   risks ①③). Fix any failures first; everything else already landed, no need to re-run the
   whole thing.
5. **Change the password** — open the gateway in a browser; first login forces a redirect to
   the change-password page.
6. **Verify the Sidebar entry by eye** (if that step wasn't skipped) — go back to the device's
   main screen, confirm the expected entry shows up under KOReader in the sidebar, and click it
   to confirm it actually launches — no script can confirm this step for you.

Walking through the manual prerequisites first, then installing everything with one command,
then eyeballing the UI-level changes last, is the order with the fewest surprises so far; doing
it the other way around (install everything first, only later discover some manual prerequisite
was missing) tends to look like "a few steps silently failed" and takes longer to debug.

## Install everything with one command

Connect your computer to the device over USB (reMarkable configures itself as `10.11.99.1` on
that link by default):

```sh
git clone https://github.com/bbq191/rm-tweak.git
cd rm-tweak/packaging
sh install-all.sh 10.11.99.1
```

This runs the following steps in order (each can also be run on its own — see "Installing only
part of it" below):

| Step | What it does | Prerequisite |
|---|---|---|
| Firmware safety gate | Checks the device's firmware against the verified version | — |
| Domestic NTP | Swaps chrony's servers for reachable ones (Aliyun/Tencent Cloud, etc.) | — |
| Anti-autosuspend wakelock for time sync | Holds a wakelock for the first minute after boot so autosuspend can't interrupt chronyd's first sync | — |
| Default timezone | Sets Asia/Shanghai | — |
| Battery diagnostics | A resident sampling service, viewable under the web UI's Manage → Battery Detective | — |
| xovi boot-persistence | Installs a unit that re-runs `xovi/start` automatically on every boot, so you no longer have to do it by hand after a reboot | requires `vellum add xovi` |
| Precise highlight snapping | Precise CJK highlight-snapping (snaps exactly what you drag, not "drag a bit, snap the whole line") | same |
| Handwriting stroke rendering | Tunes stroke thickness by pen angle/speed | same |
| Sidebar entry | A shortcut straight to "KOReader" in the sidebar; if the third-party WeRead app is installed, adds a "WeRead" entry too | requires `vellum add qt-resource-rebuilder appload` (see risk ① below) |
| Shelf + gateway + notes + fonts/wallpaper | Nine web services: book management, note ingestion/transcription, upload-and-use fonts/wallpapers | — |
| Apply | Once the xovi extensions above are staged, runs `xovi/start` exactly once at the end to make them take effect | — |

When it finishes, it prints a summary: what got installed, which step (if any) failed, and what
still needs to be done by hand (vellum bootstrap, KOReader sideloading, etc.). No step is
retried automatically or silently skipped on failure — just follow the error message. Every
script is idempotent, so re-running the whole command is always safe.

## After installing

Open `https://10.11.99.1/` in a browser (or `https://shelf.local/` on the same network segment —
note Android doesn't resolve `.local` domains):

- Default password is `shelf`; **you must change it on first login** (the system forces a
  redirect to the change-password page).
- You'll see an untrusted-certificate warning (self-signed cert) — the login page has a
  "Download CA certificate" link; install it into your browser/system trust store once to stop
  seeing the warning, or just click through "Advanced → Proceed" for a one-off visit.

## Firmware safety gate

Before installing anything, `install-all.sh` SSHes into the device, reads the sha256 of
`/usr/bin/xochitl`, and compares it against the verified hashes recorded in
`packaging/firmware-allowlist.txt` — it only proceeds on a match, refusing otherwise. This is
strict because features like the font menu and the trash/new-folder proxy rely on **byte-level
QML injection offsets** — even a hotfix that keeps the same version string can shift the
internal layout, so a version string alone isn't a safe enough guarantee.

If you've verified this exact firmware yourself and just need to register its hash: pass
`--force` (it appends the current hash to the allowlist automatically).

```sh
sh install-all.sh 10.11.99.1 --force
```

## Installing only part of it / skipping steps

```sh
sh install-all.sh 10.11.99.1 --skip chrony-cn,timezone-cn,xovi-persist
```

Skippable step names: `chrony-cn`, `chrony-boot-wakelock`, `timezone-cn`, `battop`, `xovi-persist`, `hl-snap`,
`handwriting-stroke`, `sidebar-entry`, `shelf`, `xovi-apply`. Each step's underlying script
(`packaging/deploy-<step>.sh <host>`) can also be run on its own, independent of
`install-all.sh`.

## What this installer deliberately does not do

- **Doesn't install vellum/xovi/qt-resource-rebuilder/appload themselves, doesn't sideload
  KOReader** — see "Before you install" above; these remain manual prerequisites.
- **Doesn't install the Chinese input method** — that line isn't included in this repository
  (see the top-level [README](README.en.md), "History and scope") and isn't distributed by this
  installer.
- **Doesn't install wifi-watch** (automatic WiFi carrier-loss reconnection).
- **No symmetric one-command uninstall** — `shelf/uninstall.sh` can remove the shelf portion;
  everything else is removed by hand with `systemctl disable --now <unit>`.

For what each step does in detail and its common failure causes, see the comments inside each
`packaging/deploy-*.sh` script; this document is meant as a quick-start guide for a first
install.

## Risk warnings: which modules are prone to trouble

These aren't "random low-probability glitches" — they're known issues with clear trigger
conditions. Knowing about them ahead of time saves a lot of guessing later.

① **The official AppLoad release (v0.5.3) has a compatibility issue on 3.28 firmware, and it
   fails silently — but it won't fail to install, won't stop xochitl from starting, and won't
   brick the device.** AppLoad's own built-in injection patch targets 3.27's old UI anchors,
   which were renamed in 3.28 — without a third-party compatibility patch, the launcher
   component AppLoad injects into the UI never gets built, and the device log records a
   low-level parsing error. Verified on real hardware: **`vellum add appload` itself installs
   successfully** (a plain file-level install that doesn't check firmware version), **and
   xochitl starts and works normally** — the only observable symptom is the AppLoad icon never
   showing up / not being clickable. That's "one feature didn't take effect", not "failed to
   restart" and definitely not "bricked the device". The kind of issue that actually can brick a
   device or prevent it from booting (breaking the system's own startup dependencies into a
   deadlock) is a completely different category of accident from a missing QML anchor.
   **Symptom**: the `sidebar-entry` step detects this automatically and skips (not an error, not
   a failed install) — the sidebar simply won't show a KOReader/WeRead entry, which is easy to
   mistake for "this feature was never built". **How to tell if you hit this**: check whether
   `install-all.sh`'s summary lists `sidebar-entry` as "installed" or "skipped"; if skipped and
   you actually need the shortcut, you'll need to sort out AppLoad's compatibility with this
   firmware version yourself first (the community has patches for this; it isn't something this
   installer can do for you) and then rerun.
② **Missing `qt-resource-rebuilder` silently disables several unrelated-looking features at
   once, easy to mistake for a broken install.** The font-menu enhancement, the trash/new-folder
   web proxy, and the Sidebar entry — three otherwise-unrelated features — all share the same
   prerequisite (`vellum add qt-resource-rebuilder`). Miss this beforehand, and you'll find
   several seemingly-unrelated features missing at once, easy to suspect something actually
   failed; in reality they're all the same root cause, and `install-all.sh`'s summary marks each
   one "skipped", not "failed".
③ **Restarting/stopping-and-starting xochitl repeatedly in a short window risks tripping the
   crash-loop protection — it doesn't matter who triggers it.** The trigger condition doesn't
   care *who* asked for the restart — this repository's own deploy scripts
   (`hl-snap`/`handwriting-stroke`/`sidebar-entry`, when run standalone, not through
   `install-all.sh`, each run `xovi/start` on their own), a third-party installer like `vellum
   add appload` restarting things on its own, or an app like WeRead that takes over the screen
   on launch and hands it back on exit (each stopping and starting xochitl once per round-trip)
   — all count. Verified on real hardware: just two restarts in quick succession actually
   tripped this once and triggered an unplanned full device reboot — **and that reboot was just
   the device rebooting and coming back up fine, not a brick**, purely a few extra minutes of
   waiting, not lost data or a damaged device. `install-all.sh` already handles this correctly
   for its own steps (stage everything first, run `xovi/start` exactly once at the end) — **this
   only needs your attention when you run deploy scripts by hand one at a time, or go back and
   forth between third-party apps like appload/WeRead that restart xochitl on their own** —
   leave a few minutes between each, confirm the previous restart has settled, before doing the
   next thing. Don't stack them back-to-back.
④ **The firmware safety gate refusing to install isn't a bug — it's working as designed. Verify
   before you `--force`.** Features like the font menu and the trash/new-folder proxy rely on
   byte-level QML injection offsets — a matching version string doesn't guarantee the internal
   layout hasn't shifted (a silent hotfix can move it). When refused, first confirm this device's
   firmware really is identical to the one you verified before deciding to `--force` — forcing an
   install on an unverified firmware risks a feature silently not working, or worse, affecting
   normal device operation.
⑤ **The screen will flash / the UI will restart several times during install — this is
   expected.** Every `xovi/start` fully restarts xochitl (the compositor and UI process). Don't
   use the device while installing; wait for the closing summary to print before touching it.
⑥ **There's no symmetric one-command uninstall** — see "What this installer deliberately does
   not do" above. If you change your mind about a step after installing it, cleanup is currently
   manual — there's no `uninstall-all.sh` to undo everything at once. Deciding whether you want a
   feature before installing it is cheaper than regretting it afterward.

## After a firmware update (OTA)

An OTA only wipes the device's system partitions (`/usr` + `/etc`); everything under `/home`
(book masters, configuration, certificates) is preserved as-is. After updating, just re-run:

```sh
cd packaging && sh install-all.sh 10.11.99.1
```

to restore everything — every script is designed to be idempotent, so running it again is
never harmful.

## Running into trouble

Start with the summary `install-all.sh` prints at the end to see exactly which step failed; the
corresponding `packaging/deploy-*.sh` script has detailed comments explaining what that step
does and its common failure causes.
