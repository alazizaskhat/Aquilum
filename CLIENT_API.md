# Embedded client API

**Installed and verified on 2026-10-23:** Aquilum was updated and restarted with the embedded backend. Settings → Backend is visible in the running app and reports Running. Existing HTTPS clients, note reads/search, token creation and immediate revocation were verified. The old standalone gateway task was removed; the Aquilum sign-in task now starts the combined app.

The combined build runs the authenticated notes API inside Aquilum. It calls the Rust core directly, with no separate backend process, MCP client connection or second search index. IIS continues to expose `https://test1.salemsmart.kz/notes-api/` and forwards it to the app's loopback listener at `127.0.0.1:8790`.

## Operation

Open Aquilum normally. Its client API starts with the app and stops when the app closes. A single Windows task named `Aquilum` starts the normal, non-administrator app at sign-in and is configured to restart it after a failure. The old `Aquilum Notes Gateway` task is removed after the migration checks pass. No Windows password is stored. Closing the app intentionally also stops the notes API; reopen it to resume access.

The migration preserves the existing public URL and client tokens. The embedded API does not depend on the MCP switch or MCP credentials. MCP remains available separately for AI clients, and its write setting does not enable writes on the client API.

Configuration lives with Aquilum:

```text
%APPDATA%\com.dmitriy.aquilum-app\client-api\config.json
%APPDATA%\com.dmitriy.aquilum-app\client-api\tokens.json
```

The directory is restricted to the Windows user and SYSTEM. `tokens.json` contains client names, hashes and revocation flags, without raw credentials. Token changes apply to new requests immediately. Use **Settings → Backend** to enable or disable the API, check its status and last error, restart it, select the notes folder, change its port and request limits, and manage client tokens. Settings changes are validated, saved in `config.json`, and applied through a background backend restart; Aquilum stays open. Invalid settings leave the previous configuration and listener intact. For manual edits to `config.json`, use the Backend restart button to reload them. A missing config leaves the API off; invalid config or credentials fail closed.

Create a client token in **Settings → Backend → Clients** before enabling a new backend. The save dialog creates a private token file; its filename identifies the client. Existing files are never overwritten. Store the credential on the intended device and use **Revoke** to withdraw access immediately. The settings screen displays client names and revocation state without exposing credentials or hashes.

Choose a notes folder that has already been opened in Aquilum. This keeps access tied to an explicitly selected knowledge base. The listener always remains local. Changing the backend port also requires changing the IIS forwarding destination; the public URL is managed by IIS.

Example configuration:

```json
{
  "enabled": true,
  "bind": "127.0.0.1:8790",
  "workspace": "C:\\Users\\alazi\\Documents\\notes",
  "tokens_file": "tokens.json",
  "allowed_origins": ["https://test1.salemsmart.kz"],
  "requests_per_minute": 120,
  "burst": 20
}
```

Workspace access remains fixed by the local configuration. All endpoints require a client bearer token, only read operations are permitted, and request limits, path restrictions, origin checks and no-store headers remain in place. IIS rejects plain HTTP on this route.

## Client endpoints

Send `Authorization: Bearer <client-token>` over HTTPS.

| Path after `/notes-api` | Parameters |
|---|---|
| `/health` | None |
| `/notes` | `folder`, `q`, `limit`, `offset`, `sort=path` or `links` |
| `/note` | Required `path`; optional `heading`, `limit`, `offset` |
| `/export` | Required `path`; complete UTF-8 Markdown snapshot, maximum 8 MiB |
| `/outline` | Required `path` |
| `/search` | Required `q`; optional `folder`, `limit` |
| `/backlinks` | Required `path` |

Use full relative `.md` paths and URL-encode query parameters. Follow `nextOffset` for pagination. If `indexing` is true, search results can be incomplete. The existing initial token remains in the original private `aquilum-gateway/data/secrets/first-client.token` delivery file; it was not regenerated during migration. Store it in each trusted client's secure storage rather than in public source or a distributed app bundle.

## Connect another Aquilum desktop

Use a build with **Settings → Connections** on the client PC and a host build that supports `/export`. Pushing source to GitHub does not update installed applications. The folder-structure fix and organizer are client features, so updating the client is sufficient when the host already supports note copying.

On the host PC, enable **Settings → Backend**, choose the shared vault, and create a client token for the other computer. Transfer the private `.token` file to that computer. The host must be reachable through its configured HTTPS proxy; the loopback address shown in Backend works only on the host itself. For the existing deployment, use `https://test1.salemsmart.kz/notes-api/`. Other installations need their own HTTPS proxy or HTTPS VPN endpoint; this feature does not configure forwarding, certificates or firewall rules.

On the other PC, open **Settings → Connections**, enter the full API base address, choose the transferred token file, and click **Connect / Refresh**. Browse the paginated note list or filter by title. **Read** fetches a read-only text preview, with paging for long notes and **Refresh** for newer content. Return with **Back to notes**. The app remembers the address and token-file location in `connections.json`; it does not copy the credential into settings or connect automatically at startup.

With a local vault open, use **Copy locally** on a note or **Copy all notes**. Notes go directly into their original relative paths in that vault: for example, `Learning/Module 01/Overview.md` stays at that path. This keeps folders and full-path wiki links together when copying notes individually or as a batch. Identical notes are skipped. Existing local notes are never overwritten; differing remote versions are saved alongside them as `Overview (remote copy).md`, with numbered names for further versions. Repeating a copy does not create extra folders or duplicate identical versions. Copy all ignores the title filter. Both PCs must stay online and the host app must remain running. A host built with the new `/export` endpoint is required for copying; older hosts can still supply browsing and paginated previews.

If an earlier build created many `Imported-…` folders, open that local vault and use **Settings → Connections → Organize previous copies**. This works offline and combines all old UUID-named import folders into the original folder tree. Identical notes are deduplicated and differing versions are retained. After verifying each folder, Aquilum moves its original contents to Trash, where they can be restored. Folders with unexpected files, attachments, links or changing contents are left in place and reported for manual review. The organizer does not run automatically.

This first version copies Markdown notes only. Attachments, books, history, settings and automatic two-way synchronization are not included, so embedded media may be unavailable in copied notes. Each note is fetched as a single bounded snapshot (up to 8 MiB); the whole vault is not locked against concurrent changes. Unsafe paths, redirects and plain HTTP on non-loopback addresses are refused. Requests run in the background with timeouts and pacing for the default API limit. **Disconnect / Cancel** stops further work after any in-flight request finishes; successfully copied notes remain in the local vault, and errors show the completed counts and destination so partial copies are visible. Revoke the token on the host to stop subsequent requests immediately.

## Create or revoke a token with Aquilum

The Windows app is a GUI executable, so use `Start-Process -Wait` for these commands. Token management exits without opening another app window. No raw token is printed.

```powershell
$aquilumExe = Join-Path $env:LOCALAPPDATA 'Aquilum\aquilum-app.exe'
$privateTokenFile = Join-Path $env:APPDATA 'com.dmitriy.aquilum-app\client-api\phone.token'
$command = Start-Process -FilePath $aquilumExe -ArgumentList @(
    '--api-token-create', 'phone', ('"' + $privateTokenFile + '"')
) -WindowStyle Hidden -Wait -PassThru
if ($command.ExitCode -ne 0) { throw 'Token creation failed.' }
```

The output file must not already exist. Copy its credential to the device's secure storage. Revoke the client without restarting Aquilum:

```powershell
$command = Start-Process -FilePath $aquilumExe -ArgumentList @(
    '--api-token-revoke', 'phone'
) -WindowStyle Hidden -Wait -PassThru
if ($command.ExitCode -ne 0) { throw 'Token revocation failed.' }
```

## Source and validation

- `aquilum-app/gateway`: shared Axum API, token security and embedded runtime. Aquilum links it with default features disabled, so the MCP client SDK and standalone process are not part of this build.
- `aquilum-app/core/src/client_api.rs`: connects API operations directly to the core and manages startup/shutdown, validated live configuration and status events.
- `aquilum-app/core/src/connections`: outbound client, background browsing/copying, cancellation and integration tests.
- `aquilum-app/native/src/main.rs`: token management commands.
- `aquilum-app/native/src/ui/settings/schema/backend.rs`: Backend settings UI, with English and Russian labels.
- `aquilum-app/native/src/ui/settings/schema/connections.rs`: remote browser and read-only preview, with English and Russian labels.

The core regression suite passed (697 tests; six existing tests skipped), and the embedded API suite passed (10 tests). The desktop regression suite passed (450 tests; 40 existing tests ignored), and the additional Backend settings action test passed. Twelve rendered English/Russian previews cover status, connection settings and client access in both themes. The release executable is 31.46 MiB. See `backend-settings/validation.json` in the integration workspace for the validation record. Existing vendored Krilla deprecation warnings remain unchanged.

The installer backs up the previous executable and legacy task definition. It checks existing client access over HTTPS, verifies token creation and immediate revocation through the Aquilum executable, and verifies that the API port belongs to Aquilum before removing the old task. Its result is recorded in the integration workspace's `install-status.json`.

Connection feature validation: the desktop release build succeeds; 706 core tests and 454 desktop tests pass (6 core tests and 45 desktop tests ignored). The unchanged embedded API suite previously passed all 11 tests. Coverage includes authenticated browsing, filtered pagination, remote refresh, complete Unicode copies beyond the preview limit, preservation of local notes, token revocation, cancellation and oversized snapshot rejection. Copy regression tests verify the original folder tree, full-path wiki links, deduplication, differing versions, offline organization of earlier imports, restoration from Trash and retaining originals that change during organization. Twelve English/Russian light/dark previews were rendered in `.artifacts/structured-copy-preview`. Existing vendored Krilla deprecation warnings remain.
