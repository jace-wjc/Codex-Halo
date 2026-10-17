# Codex Halo

Native-feeling desktop quota monitor for the local Codex Desktop login state, with a compact floating widget and a synchronized macOS menu bar panel.

## Highlights

- Shows your Codex plan, every usage window currently returned by Codex (including weekly-only or 5-hour + weekly modes), next reset time, and reset-credit availability.
- Displays compact live quota text in the macOS menu bar; Windows falls back to a tray icon, tooltip, and context menu.
- Uses clear quota states for healthy, caution, and critical remaining usage.
- Collapses into a small floating orb when idle, then expands on hover.
- Opens a dedicated quota panel from the menu bar with refresh frequency, manual-only mode, widget visibility, always-on-top, click-through, launch-at-login, language, and quit controls.
- Uses English on first launch by default, with Simplified Chinese available from the language setting.
- Keeps the menu bar, detail panel, and floating window synchronized through one Rust-owned state coordinator.
- Refreshes quota every five minutes by default, with presets from 10 seconds to 30 minutes, a validated 10-second-to-24-hour custom interval, or manual-only mode. Reset boundaries can accelerate slower schedules to one minute, while failures use bounded exponential backoff; manual refreshes remain serialized and debounced.
- Shows reset credit count and available reset-credit expiration times when the quota service provides them.
- Handles stale data, signed-out sessions, unavailable quota responses, and loading states without fabricating values.
- Usage Stats Overview reads the same official account statistics as Codex (total tokens, peak day, sessions, streaks and daily/weekly/cumulative activity). Models keeps deduplicated local model details with exact hover values and 7-day, 30-day, and all-time ranges.

## Repository Metadata

Suggested repository description:

```text
Windows/macOS menu bar and floating desktop monitor for Codex quota from the local Codex Desktop login state.
```

Suggested topics:

```text
codex, quota, tauri, react, rust, desktop-app, windows, macos, productivity
```

## How It Works

Codex Halo reads the existing Codex Desktop login state on your machine and queries Codex/ChatGPT quota and account-statistics endpoints with that session. The existing authentication reader, endpoint parser, and privacy boundary remain in the Rust desktop process. Quota values come only from normalized service snapshots; local token counts are displayed separately as usage statistics and are never used to estimate remaining quota. The app does not redeem reset credits or modify account settings.

Overview reads the official account profile usage endpoint and preserves its summary, cutoff date and activity buckets. An unavailable official response is shown explicitly rather than replaced with local totals.

Models is built on demand from metadata and token counters in `CODEX_HOME/sessions` and `CODEX_HOME/archived_sessions`, including `.jsonl.zst` compressed history. Plain/compressed copies share a logical index entry. Forks' leading replayed counters are matched against their actual ancestors without relying on rewritten timestamps; independent and sibling sessions remain separate, and real requests after counter resets are retained. Copies of one session are deduplicated with occurrence counts so repeated legitimate requests are not lost. Local totals can differ from account totals. The versioned cache automatically rebuilds older indexes, verifies previously indexed content before appending, and rebuilds rewritten files; compressed history is decoded inside the application without requiring an external zstd installation. Overview shows the official statistics cutoff date when supplied. Conversation text and tool output are never retained or uploaded.

The detail-panel refresh action updates quota, official statistics and the local usage index together. Configurable background scheduling remains quota-only; statistics revalidate on demand after 60 seconds or when refreshed manually. Statistics requests use their own lock and do not change the quota scheduler. Changing the quota interval restarts the timer without issuing an immediate request. Manual-only mode disables periodic network requests after the initial startup read.

The production web entry never substitutes mock quota. A development-only `?designer` route contains explicit visual fixtures for component work. Real quota reading requires the Tauri desktop app and an existing Codex Desktop login on the same machine.

## Download

For normal users, download the latest unsigned build from GitHub Releases:

- Latest release: https://github.com/xhzwjc/Codex-Halo/releases/latest
- Windows: `Codex Halo_<version>_x64-setup.exe`
- Windows enterprise deployment: `Codex Halo_<version>_x64_en-US.msi`
- macOS Universal: `Codex Halo_<version>_universal.dmg`

Run the Windows setup executable, or open the macOS DMG and drag Codex Halo into Applications. Unsigned builds may trigger Windows SmartScreen or macOS Gatekeeper warnings. GitHub also adds its own source-code ZIP and TAR archives to every release; those are not application installers.

### macOS says Codex Halo is damaged

The current macOS build is unsigned and not notarized, so Gatekeeper may report that the app is damaged. If you downloaded the DMG from the official Codex Halo Releases page, drag Codex Halo into Applications and run this command in Terminal:

```bash
sudo xattr -r -d com.apple.quarantine "/Applications/Codex Halo.app"
```

Enter your macOS login password when prompted, then open Codex Halo again. This command removes the quarantine attribute from Codex Halo only; it does not disable Gatekeeper globally. Do not use it for an app downloaded from an untrusted source.

## Feedback

Please use GitHub Issues for bugs, compatibility reports, and feature requests:

https://github.com/xhzwjc/Codex-Halo/issues

## Privacy Boundary

Codex Halo is local-first and intentionally narrow:

- Reads the local Codex Desktop login state only to query Codex quota.
- Sends the existing Codex access token only to ChatGPT quota and account-statistics endpoints.
- Stores widget preferences and a local aggregate usage index in its own app config directory.
- The versioned usage index contains session identifiers, counter fingerprints, timestamps, model totals and the selected source directory; it contains no conversation text or tool output.
- Does not store Codex tokens, account IDs, prompts, chat history, raw quota responses, or local auth paths.
- Does not include telemetry, analytics, crash reporting, or third-party tracking.
- Does not redeem reset credits or modify account settings.

See [PRIVACY.md](PRIVACY.md) and [SECURITY.md](SECURITY.md) for the full boundary.

## Accuracy Boundary

Codex quota is read from Codex/ChatGPT quota service responses. If the response format changes, the app shows an unavailable or stale state instead of inventing quota values.

## Development

Requirements:

- Node.js 20+
- Rust stable
- Tauri 2 system dependencies for your platform

```bash
npm install
npm run dev
npm run test
npm run build
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

On Windows, Tauri may download WiX to create an MSI installer. If WiX download fails, the release executable may still be produced at:

```text
src-tauri/target/release/codex-halo.exe
```

## Release

GitHub Actions are configured for:

- CI on push/PR: frontend tests, TypeScript/Vite build, dependency audit, Rust tests, and Tauri builds.
- `v*` tags: direct unsigned Windows NSIS/MSI installers, a macOS Universal DMG, and a public GitHub Release.

After feature changes are committed and `main` is clean, publish a version with one command:

```bash
npm run release -- 0.2.2
```

The command synchronizes every application version, runs frontend and Rust checks, creates the release commit and annotated tag, and atomically pushes both to `origin`. If the requested version is already synchronized, it tags the current clean commit without creating a redundant version commit. GitHub Actions performs the Windows/macOS installer builds and publishes the Release. Use `--dry-run` to validate a release without changing files or Git state.

See [docs/GITHUB-RELEASE-CHECKLIST.md](docs/GITHUB-RELEASE-CHECKLIST.md) before publishing a version for others.

Do not upload local credentials, `.codex`, `.env*`, screenshots with personal data, `node_modules`, `dist`, `src-tauri/target`, or local installers to source control.

## License

MIT
