# Direction routing validation

The feature is developed on `codex/device-direction-routing` across the server,
core and clients. It is a coordinated protocol-3 upgrade, not a production
rollout. Existing installed clients require the previous server release.

## Server

- `cargo fmt --all --check`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo test --locked --all-features`: 44 passing tests.
- New regression tests were observed failing in disposable copies with behavior
  disabled or assertions inverted, then restored. Actual pre-fix failures also
  reproduced ACK-lost fallback, delayed/paused ACKs, promoted-candidate expiry,
  and idempotent retry accidentally converting live delivery into backfill.
- Native HTTP tests cover protocol gating, device ownership, body authorization,
  session CAS/tombstones, pause/revoke, 30-day expiry before activity refresh,
  fixed per-device TTL, zero disabling replay, settings validation/repair/audit,
  restart lease reset, cascaded device deletion and conditional ACK cleanup.
- A real loopback WebSocket test covers control ACKs, HTTP exact-operation retry,
  stale sessions and old connection cleanup.
- `node scripts/admin-browser-test.mjs`: real disposable HTTPS Chromium passes
  administrator authentication, settings persistence/audit, CSRF, XSS-safe
  labels, session replacement, logout and account-wide token revocation.
  Desktop/mobile/dark screenshots are generated under `target/admin-ui/`;
  management UI was inspected at desktop and 390-pixel mobile widths.

## Shared core and clients

The server CI pins core commit
`9d80fac36170216436bd6e14db019c12e7dc4817` (crate 0.4.0).
Its 61 Rust tests, strict Clippy and rustdoc pass. The interoperability fixture
runs the actual Rust core and this server over HTTPS twice: real WSS direction
control with zero HTTP direction PUTs, then intentionally blocked WS with
HTTP fallback. Registration, latest-only delivery, idempotency, restart,
sensitive-content handling and conflict behavior pass in both modes.

Desktop and Omaclipsync vendor exports match that commit byte-for-byte and by
SHA-256. Desktop has 29 passing normal Rust tests and 4 passing Linux native
checks under isolated HOME/XDG, Xvfb, D-Bus and test keyring. Qt Linux builds,
profile-isolated startup and window activation/closure pass; Windows Rust type
checks/Clippy pass. Omaclipsync has 34 passing Rust tests, JS tests and QML lint.

Android compiles its application and instrumentation Kotlin and passes its
sync-specific JVM tests. An opt-in JVM interoperability test runs the actual
Rust server with temporary SQLCipher storage behind a TLS/WSS proxy. Both real
WSS and forced HTTP fallback execute direction ACK, upload, body receive,
ACK cleanup, push-only filtering, pause and logout. See the Android repository
for the command and opt-in environment variable. It does not use a phone,
production service, system clipboard or real Keystore.

## Practical limits

No production container, installed plugin, desktop clipboard or login keyring
was modified. There was no physical Android, Wayland/Sway or WinUI runtime
acceptance. Android's broader suite has an unrelated existing
`ThemeSerializationTest.version2` failure: the theme version is 2.1 while its
fixture still expects the old 2.0 migration behavior. Sync-specific checks pass;
that broader-suite failure is not reported as a successful full-suite run.

Counters measure routing decisions (backfills selected), not proof that a user
read a clipboard. Only a client ACK confirms its reported handling outcome.
