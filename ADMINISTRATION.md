# Device and account administration

The optional `/admin/` page manages access to one shared clipboard. It does not display clipboard text or device tokens. HTML, CSS, JavaScript and icons are served by the Rust binary; there is no frontend service, CDN, analytics or third-party font request.

## Enable with Docker Compose

The single `docker-compose.yml` includes browser administration and enables it by default. Keep `password.secret` for client authentication. Alongside the existing sync/database settings, set the independent administrator credentials and public origin in the local `.env` file (choose an administrator name different from `RCS_USER` and any existing device account):

```dotenv
RCS_ADMIN_USER=administrator
RCS_ADMIN_PASSWORD='replace-with-a-unique-strong-password'
RCS_WEB_ORIGIN=https://sync.example.com
```

Use a unique password of at least 12 bytes. Single-quote the password in `.env` so Compose does not interpolate `$` characters. Protect this file and recreate the container after changing credentials:

```bash
chmod 0600 .env
docker compose up -d
```

Open `https://sync.example.com/admin/`. Forward `/admin/`, `/api/admin/` and the existing public/API paths through the HTTPS proxy. The proxy must preserve the original `Host` header. The backend remains published only on `127.0.0.1:8090`; no additional public port is needed. Cloudflare must not cache the management paths; responses carry `Cache-Control: no-store`.

`RCS_WEB_ORIGIN` is one exact HTTPS origin, including a non-default port if used, without a URL path, credentials, query or fragment. Alternate hosts are deliberately rejected for administration. The backend does not trust `X-Forwarded-Host`, `X-Forwarded-For` or `CF-Connecting-IP` to grant access or bypass rate limits. TLS terminates at the trusted proxy/tunnel; never publish the backend's plain HTTP port to untrusted networks.

To disable the browser page, set `RCS_WEB_ENABLED=false` in `.env` and run `docker compose up -d`. Keep the required origin and administrator settings in `.env`; the independent administrator remains configured for native device management and pairing. Stored device accounts, registered devices and synchronization remain valid.

### Migrating an existing Compose deployment

Back up the data and configuration, then replace `docker-compose.yml` with the merged template. Preserve the existing `.env`, `password.secret` and database key, and add any missing administrator/origin values. Remove `docker-compose.admin.yml` from explicit `-f` arguments, `COMPOSE_FILE` or the 1Panel Compose configuration; the overlay has been removed from this repository. Run `docker compose up -d` with the single file. No data-volume change, credential reset or device re-pairing is required by this configuration merge.

## Accounts and permissions

| Credential | Pair clients and synchronize | Browser administration / Basic device management |
| --- | --- | --- |
| Independent administrator | Yes, on multiple devices | Yes |
| Existing `RCS_USER` account | Yes, with unchanged credentials | Only when no independent administrator is configured |
| Device account created in the page | Yes, shared by multiple devices | No |
| Device bearer token | Synchronize as its registered device | No |

All accounts share the same current clipboard and revision stream. They are access-control groups, not separate users or separate clipboards. The administrator is unique and configured on the server, not created in the page. Environment-configured administrator and existing sync accounts cannot be removed from the page. When independent administrator credentials are configured, only those credentials authorize the native `GET /api/devices` and `DELETE /api/devices/{deviceId}` management endpoints. Existing device identities/tokens remain stored, but synchronization now requires protocol 3 and direction/session negotiation; old clients must use the previous server release.

Use **Create account** to set a device username and password. Usernames contain 1–64 ASCII letters, digits, `.`, `_` or `-`. Passwords are 12–4096 UTF-8 bytes and cannot be retrieved after creation. A browser-generated password uses the Web Crypto random source. Save it before creating the account. The server stores an independently salted Argon2id PHC hash in the SQLCipher database, never the clear password. Argon2id uses 19 MiB, two iterations and one lane. Only one password hash/verification runs at a time, on a blocking worker; the semaphore permit stays with the work even if its HTTP request is cancelled. Excess password work returns `429` instead of blocking sync requests or allocating unbounded memory. There can be at most 64 managed accounts and 32 registered devices overall.

**Revoke device** removes just that device token. Other devices under its account keep working. Because its account password remains valid, the device can explicitly pair again. **Revoke account** atomically deletes the account password hash and all devices registered to it, then wakes pending long polls. That password can no longer register devices. An in-flight registration rechecks the account inside its database transaction to prevent resurrection after revocation. Clipboard history is retained under the normal cleanup policy; revocation cannot erase content already delivered to a client.

If an administrator password is compromised, change its configured password and recreate the container, and revoke affected devices. Changing a password alone is not the same as revoking issued device tokens. Restarting invalidates browser sessions.

## Presence and labels

Device labels are administrator-written metadata, not device-reported identity. Always inspect the complete device ID before revoking. Labels are at most 80 Unicode characters, cannot contain controls, and are rendered as plain text.

**Online** means the server received a valid bearer-authenticated request within the past 60 seconds during the current process lifetime. It is not a reachability check and may show an idle upload-only client as offline. Last activity uses server time, is updated in memory for each authenticated request, and is written to the encrypted database at most once per minute per active device. Existing devices initially show “Not recorded.” After a restart, persisted timestamps remain available but do not themselves imply online status. Up to 60 seconds of recent activity can be lost on a crash.

The page refreshes metadata every 15 seconds while visible. Background refresh does not extend the session's idle timeout; user interaction sends a throttled authenticated keepalive. Hiding the page stops refresh work until it is visible again.

## Sync state and settings

The page defaults to English, with a Chinese language switch. Use **Devices**, **Accounts**, **Settings** and **About** to navigate. The GitHub header link opens the server repository in a new tab with `noopener noreferrer`; it loads no external assets and sends no referrer.

**Devices** shows send-only, receive-only or bidirectional sync, with pause displayed separately. Report state is **Accepted by server**, **Direction not reported**, **Direction revoked** or **Direction expired**. Unknown/revoked/expired directions receive nothing. Acceptance means the server stored the report, not that the client received its ACK. Hover or keyboard-focus **Sync details** to read its explanatory tooltip; touch users can expand the same explanation. Expand the details for the last direction operation time/source/version, activity expiry and current data lease. The last report source is not the current connection transport. A direction expires after 30 days without activity; this is separate from token revocation. The 30-second data lease is also distinct from the 60-second activity badge. Acknowledged revisions include skipped messages and do not prove the clipboard was written. Reading this list does not refresh activity, TTL, direction version or acknowledgment cursor.

These fields are additive, administrator-only metadata from `GET /api/admin/devices` under `sync`: `direction`, `paused`, `reportState` (`reported`, `unknown`, `revoked`, `expired`), `reportedAt`, `expiresAt`, `lastReportSource` (`ws`, `http`, or null), `version` and `dataLeaseActive`. They expose no session ID, token or clipboard body. The page does not remotely change client direction or sensitive-content policy.

**Settings → Reconnect backfill** controls the global `sync.reconnect.backfill.window`, initially **5 minutes**, range **0–1440**. Saving applies immediately without restarting. Nonzero changes affect newly stored candidates; existing records retain their write-time expiry. **0** disables offline storage/replay and clears pending offline candidates, including promoted backfill, while current-session live delivery continues. Only the administrator can save; the actor, time and old/new values are audited. Invalid input is rejected, and corrupt runtime configuration falls back to 0 with an explicit warning. Tab changes and refresh preserve unsaved input; logout clears private settings and diagnostics.

**About → Server diagnostics** displays report/error/source, filtering, unknown-state, revocation, expiry, backfill and configuration-alert counters from `GET /api/admin/sync-settings`. Use **Clear statistics** to reset these counters after confirmation. The operation requires an administrator cookie, exact Origin and CSRF token: `POST /api/admin/diagnostics/reset` with `{"confirm":true}` returns the fresh `metrics` snapshot. Only process-local counters are reset; settings, audit records, devices, pairing, clipboard data and delivery queues are retained. New activity may immediately increase the counts. Rate limits, live connections and configuration-warning suppression are also retained. Restarting the server resets the counters too. The page preserves unsaved backfill input and rejects stale pre-reset refresh data. Backfills count selected candidates, not completed clipboard writes. The troubleshooting list checks protocol, direction/pause/report/lease, offline expiry, then local filters. Sensitive text may remain in a client's inbox without writing the system clipboard.

**About → Check for updates** compares the compiled server version from administrator-only `GET /api/admin/about` with the public [GitHub latest-release endpoint](https://docs.github.com/en/rest/releases/releases#get-the-latest-release). It runs only after a click, directly from the browser, with credentials omitted, no referrer, redirects rejected, a 10-second timeout and a 256 KiB response limit. Only validated stable numeric release tags are compared. No clipboard/device data, administrator cookie or CSRF token is sent to GitHub. Release text is never interpreted as HTML and the release link is pinned to this project's GitHub releases. A development build newer than the published release is identified as newer, rather than suggested for downgrade. Errors and GitHub rate limits are shown; no automatic container upgrade or Docker access is provided.

## Browser security

- Only the independent administrator can log in. Login requires an exact allowed Origin, JSON and a custom request header, preventing form-based login CSRF. No cross-origin CORS access is enabled.
- A new 256-bit random session ID is issued on login; only its SHA-256 digest is stored server-side. The `__Host-rcs-admin` cookie has `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/` and no `Domain`. No credentials or session tokens are placed in localStorage, sessionStorage or URLs.
- All mutations, including logout and keepalive, require the session-bound CSRF token plus exact Origin validation. GET endpoints do not mutate access permissions. Device tokens and cookies cannot substitute for each other's authentication scheme.
- Sessions expire server-side after 15 minutes of inactivity or one hour from login. Logout deletes the server session and expires the cookie. At most 16 sessions are retained; the oldest is evicted when another valid login exceeds this limit. Browser close alone is not a reliable logout mechanism.
- A bounded process-wide budget allows 30 password-authenticated requests per minute across login and Basic API endpoints. It deliberately ignores caller-supplied IP headers. Existing bearer-token sync requests do not consume this budget. Repeated failures return a generic `401`; exceeding the budget returns `429`. There is no permanent account lockout. For a public deployment, additional proxy-side throttling or Cloudflare Access restricted to `/admin*` and `/api/admin/*` can add protection without intercepting native sync routes.
- Management responses, including errors, are non-cacheable and include a CSP allowing only same-origin scripts, styles, connections and images, with no inline code, frames or external assets. The only extra connection destination is `https://api.github.com`, used solely for the explicit update check. Dynamic values use DOM text nodes. Dialogs use native focus containment and cancellation; disabling a UI button is not treated as authorization.
- Logs include login outcomes and validated IDs for creation/revocation, not passwords, token/hash values, cookies, clipboard text or request bodies. The administrator's configured secret still exists in server memory, as does the existing sync password. Root, the container runtime and a compromised server are outside this protection boundary; this is not end-to-end encryption.

## Configuration outside Docker

The binary's `RCS_WEB_ENABLED` default remains `false`; the supplied Compose template explicitly defaults it to `true`. When running the binary directly, enable it with `true` and supply `RCS_WEB_ORIGIN`, `RCS_ADMIN_USER` and `RCS_ADMIN_PASSWORD` through your service manager. Compose reads these values from the local `.env`. No separate administrator password file is needed. Environment variables can be inspected by process/container administrators; never commit `.env` or publish resolved Compose output. Remove any old `RCS_ADMIN_PASSWORD_FILE` setting when migrating an existing deployment. Administrator credentials can also be configured with the web page disabled, for native device management and pairing clients as the administrator. They must not collide with an existing managed device username or the existing sync username.

Schema additions are automatic when opening the database and preserve existing token hashes, epoch, cursors and current clipboard. Back up the complete database and key before upgrading, as with any schema change. Enabling the management page itself does not require re-pairing. Protocol-3 client migration is a separate coordinated upgrade; see PROTOCOL.md.

## Developer verification

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked
node scripts/admin-browser-test.mjs
```

The browser test needs Node 22+, OpenSSL and Chromium (`CHROMIUM_BIN` can specify the executable). It creates a temporary SQLCipher database, HTTPS proxy and isolated browser profile with the basic password store. Certificate checks are bypassed only inside this disposable test browser and the test HTTP client. It never uses the desktop keyring, clipboard, production service or an existing browser profile. Screenshots under `target/admin-ui/` contain only synthetic fixture data. CI also runs the fixed shared-core HTTPS interoperability probe against the same server build.
