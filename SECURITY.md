# Security

## Supported Use

Codex Halo is a local desktop utility that reads Codex quota using the user's existing Codex Desktop login state and safely distinguishes API-key authentication from ChatGPT subscription authentication.

## Reporting Issues

Please do not open public issues containing tokens, account IDs, raw backend responses, screenshots with personal data, or local file paths. Redact sensitive information before sharing logs or bug reports.

## Security Boundaries

- The app does not persist Codex credentials.
- The app does not log request headers or raw quota responses.
- The app caps auth file reads at 256 KB and quota responses at 1 MB.
- The app does not follow redirects for quota HTTP requests.
- The app does not redeem reset credits or change account settings.
- The standard project API key is never sent by Codex Halo or exposed to the frontend.
- Optional `OPENAI_ADMIN_KEY` support is environment-only, remains in the native process, uses fixed `api.openai.com` Usage/Costs endpoints, caps pagination, and does not persist raw responses.
- Unknown Admin API response shapes fail closed instead of producing default totals. Organization cost is never represented as account balance.

## Release Notes For Maintainers

Before publishing a release, verify:

- Source archives do not include local installers, build outputs, `.codex`, QA screenshots, or environment files.
- Windows/macOS bundles are built by CI or a clean machine.
- Unsigned builds are clearly labeled as unsigned.
- Signed releases are produced only with maintainer-controlled certificates and secrets.
