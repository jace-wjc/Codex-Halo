# Privacy

Codex Halo is designed to be local-first and minimal.

## What It Reads

- The app reads the local Codex Desktop login file from `CODEX_HOME/auth.json` or the user's `.codex/auth.json`.
- The app sends the existing Codex access token only to the ChatGPT quota and account-statistics endpoints needed to read Codex usage.
- The app may read the account identifier from the login file or token payload only to set the request header expected by the quota service.
- The app recognizes API-key authentication from Codex's local `auth_mode` and credential metadata. It does not use the normal project API key for its own requests, expose it to the webview, persist it, or log it.
- Optional organization monitoring reads `OPENAI_ADMIN_KEY` only from the native process environment. The key is never sent to the webview or written to Codex Halo settings.
- When the user opens Usage Stats, Overview reads official account statistics. Models selects timestamps, model identifiers, session/fork metadata and token counters from local `CODEX_HOME/sessions` and `CODEX_HOME/archived_sessions` JSONL and Zstandard-compressed JSONL records. It does not retain or upload conversation text or tool output.

## What It Stores

Codex Halo stores widget preferences in its own application config directory:

- floating-widget visibility
- locked state
- always-on-top state
- pinned provider
- auto-rotate interval
- interface language

It also stores a versioned local usage index containing transcript/session identifiers, content hashes, byte cursors, dates, models and token-counter events. It records the selected Codex data-directory path to prevent mixing indexes from different directories. The index does not contain credentials, account IDs, raw service responses, prompts, responses or tool output. Compressed records are streamed in memory; source transcripts are never rewritten or decompressed on disk.

## What It Sends

The app only calls these quota-related HTTPS endpoints from the local desktop process:

- `https://chatgpt.com/backend-api/wham/usage`
- `https://chatgpt.com/backend-api/wham/rate-limit-reset-credits`
- `https://chatgpt.com/backend-api/wham/profiles/me` (official account statistics, on demand)

For an API-key-authenticated Codex session, these ChatGPT endpoints are not called. If and only if `OPENAI_ADMIN_KEY` is present, the native process additionally calls these fixed official endpoints:

- `https://api.openai.com/v1/organization/usage/completions`
- `https://api.openai.com/v1/organization/costs`

The optional results contain organization-wide aggregate token, request, and USD cost totals. Codex Halo does not store raw responses and does not call an undocumented credit-balance endpoint.

No telemetry, analytics, crash reporting, or third-party tracking is included.

## Logging

Logs are intentionally generic. They must not include tokens, account IDs, raw backend responses, request headers, local auth paths, or personal file paths.

## Accuracy Boundary

Codex Halo displays quota windows returned by the Codex quota service. It does not estimate remaining quota from local token usage or fabricate unknown values. ChatGPT Overview uses official account summaries and activity buckets; Models is a separate local view whose scope can differ due to local retention, account changes and transcript history. API-key accounts retain a local overview. Neither local statistics nor optional organization cost represents prepaid balance or remaining quota.
