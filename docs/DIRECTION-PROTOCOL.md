# Device direction protocol (implementation contract)

This is a coordinated breaking upgrade. All synchronization requests require
`X-RCS-Protocol: 3`; missing/unsupported values return HTTP 426 and JSON
`{"code":"PROTOCOL_UNSUPPORTED","minimumProtocolVersion":3}`. Responses advertise
`X-RCS-Protocol: 3`. Public health/icons and administrator session APIs are exempt.
Do not deploy over the production service until matching clients are available.

## Shared wire

All paths use `/api/`. Device bearer credentials are unchanged. Account ownership
comes from the authenticated device, never caller-supplied account identifiers.
`deviceId` and `sessionId` are canonical lowercase UUID v4. Versions are integers
0..9007199254740991. Every successful mutation advances version by exactly one;
only exact retries of the same operation/session/version/payload are idempotent.

- `GET /api/devices/{deviceId}/sync-direction`: returns
  `{deviceId,sessionId:null|string,version,direction:null|"PUSH_ONLY"|"RECEIVE_ONLY"|"BIDIRECTIONAL",paused,updatedAt,expiresAt,revoked}`.
  It never activates state or refreshes its TTL.
- `PUT /api/devices/{deviceId}/sync-direction`: JSON
  `{sessionId,version,direction,paused,connect}`. `connect:true` claims a fresh
  client-generated session with version=current+1. `connect:false` updates only
  the current session, with version=current+1. Initial device version is zero.
  New-session claims use GET then CAS; delayed claims cannot replace newer ones.
- `DELETE` on the same path: JSON `{sessionId,version}`. Only the active matching
  session can revoke. Keep a tombstone (including last operation) for retry and
  replay protection; never clear its version counter.
- Successful PUT/DELETE returns
  `{deviceId,sessionId,version,serverTime,expiresAt,revoked}` (`SyncDirectionAck`).
- Errors use JSON `{code,version?}`. Codes: `UNAUTHENTICATED` (401),
  `DEVICE_MISMATCH` (403), `VERSION_STALE` (409), `SESSION_REVOKED` (409), `SESSION_OFFLINE` (409, same active SID but expired online lease),
  `RATE_LIMITED` (429), `DIRECTION_FORBIDDEN` (403), `PROTOCOL_UNSUPPORTED` (426), `INVALID_DIRECTION` (400).
- `/api/ws` authenticates with the same Bearer and protocol header. Text frames
  are JSON `{"type":"SyncDirectionUpdate",...PUT fields}` or
  `{"type":"SyncDirectionRevoke",...DELETE fields}`. First update is connect;
  the socket then binds to that session. Replies are
  `{"type":"SyncDirectionAck",...ACK fields}` or
  `{"type":"SyncDirectionError",code,version?}`. Bound incoming messages to
  4096 bytes. Ping/Pong keepalive, no tokens in URL or subprotocol strings.
  A current enabled generation may recover SESSION_OFFLINE by GET + a fresh
  session CAS; SESSION_REVOKED never grants permission to revive an old generation.
  HTTP retries reuse the exact failed WS operation; they must not blindly open
  another session. Ordinary socket/process close means offline, not logout.

Client state is three directions plus paused (paused retains chosen direction).
Both pause and unknown/revoked deny data delivery. Receiving clients also retain
local filtering. Server rejects uploads from receive-only/paused/unknown devices.
Explicit logout revokes; network failure is reported as unconfirmed, never
pretended successful. Logout/pause increments local generation so delayed ACKs,
409s or retries cannot re-enable old settings.

After direction ACK, data requests (`state`, `changes`, `content`, upload and ACK)
include `X-RCS-Sync-Session: <accepted sessionId>`. A stale session is rejected;
registration, direction control and cancellation of uncertain uploads are exempt.
A matching paused session may ACK already-processed delivery without renewing its
receive lease; state/content/uploads remain forbidden until resumed.
The HTTP online lease is 30 seconds, renewed by accepted direction/data activity.

## Metadata and delivery

`/api/state` and `/api/changes` preserve global `revision` for conflict detection,
add nullable `deliveryRevision`, and retain the existing nullable message fields.
All message fields + deliveryRevision may be null even if global revision > 0.
When present, the message revision is deliveryRevision, not global revision.
ACKs reference that message revision. `/content` rechecks the current direction,
session and the device's authorized delivery before returning text. Source
self-delivery is suppressed. Existing local-copy conflict and idempotency rules
remain; never silently rebase an already-created/attempted upload.

## Lifetime and backfill

Direction state expires after 30 days since valid report/login/activity. Check
expiry before refreshing activity; expired/revoked state requires a new report.
Disconnect only releases online presence, retaining direction eligibility until
expiry. HTTP uses an explicitly bounded online session lease, distinct from the
admin UI's 60-second activity badge. Persistent per-device unacknowledged
candidates protect against detection gaps. Online delivery is bound to the
accepted session. Reconnect must ACK the latest direction before delivery.

Offline backfill is one row per device referencing a shared message, separate
from the global current message; no text copies per device. Overwrite only with
new eligible messages. Store fixed expiry at write time. Default window is 5
minutes, valid range 0..1440, global, persisted, admin-only, dynamically effective.
Nonzero window changes affect new writes only. Zero disables storing/replaying
and clears old backfill so 0->nonzero cannot revive it. Pause/revoke/expiry
clears pending delivery/backfill. New-device pairing does not create history.
Promoted offline delivery retains its original deadline and is removed when
backfill is disabled; live delivery is not subject to the offline window.
An issued-revision watermark authorizes delayed ACKs after pause/slot replacement
without restoring body access. ACK matching device+message+delivery revision conditionally removes delivery
and backfill; an old ACK must not erase a newer candidate. All device/account
removal and epoch rotation also clean these tables transactionally.

The setting key is `sync.reconnect.backfill.window`. Missing initialization gets
5 once; subsequent missing/corrupt/out-of-range runtime configuration uses 0 and
emits an alert. Invalid writes are rejected. Persist audit actor/time/old/new
value. Do not log tokens, session IDs, passwords or clipboard text. Metrics cover
reports/errors/source, filters/unknown, revocations/expiry, backfills, window and
configuration alerts; expose authenticated management statistics.

## Administrator diagnostics

`GET /api/admin/devices` adds a read-only `sync` object with effective direction,
pause, report state (`reported`, `unknown`, `revoked`, `expired`), report timestamp,
activity expiry, last report source, direction version and active data lease.
No session identifiers or text are returned. Reads do not renew TTL or presence.
Acceptance is not proof of client ACK receipt or a system clipboard write.
The management page has Devices / Accounts / Settings / About tabs. The global
backfill window and audit are under Settings; process-lifetime routing counters,
receive troubleshooting and version information are under About. Checking public
GitHub releases is manual, credential-free and never redeploys the container.
Native protocol-3 endpoints and client direction negotiation are unchanged.
