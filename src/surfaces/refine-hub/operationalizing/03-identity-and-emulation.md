# Authentication security & user emulation

## Four different identities

| Identity | What it answers | Required deployment decision |
| --- | --- | --- |
| Human entering Refine | Who may control the daemon? | Local-only workstation or authenticated gateway; session lifetime and revocation |
| OS process / worker | Which files, commands and network paths may execute? | Dedicated account or isolated worker; least privilege; no implicit production credentials |
| Provider account | Which model service may receive context and incur usage? | Approved tenant/account, credential storage and data terms |
| Target-system identity | Whose authority is used in an API or browser? | Scoped workload identity or explicit delegated user access with attributable actions |

Logging into a third-party application does not authenticate the human controlling Refine. Logging into Refine does not automatically authorize access to downstream applications.

## What user emulation means here

Refine orchestrates provider CLIs and tools. An agent may interact with APIs, shell commands, browser automation, or desktop tools if those capabilities are installed, configured and allowed. The reviewed source does not establish a built-in universal user-emulation engine or an end-to-end enterprise delegation system.

A browser tool using an authenticated profile can act with that profile's available authority. That is session use, not proof of the human's intent for each action. Capability depends on the selected provider, tools, platform, session state, and target application. Test the exact stack before claiming support.

For each enabled tool, document: target systems, permitted actions, acting principal, data access, approval points, session lifetime, logout/revocation behavior, and audit evidence. Use a dedicated test account/profile for demonstrations. Verify permitted operations, denied operations, expired sessions, and emergency revocation.

## Proposed third-party authentication

Use an approved identity provider for entry to Refine and an explicit delegation mechanism for downstream access. Prefer scoped, short-lived tokens or workload identities when the target supports them. Validate issuer, audience, expiry, permitted actions and revocation; protect refresh credentials. OAuth guidance supports audience restrictions and replay protection, but an IdP alone does not implement these controls inside Refine. [OAuth security best current practice](https://www.rfc-editor.org/info/rfc9700/).

Where browser-session reuse is necessary, treat the profile/cookie store as a credential. Isolate it per permitted principal, preserve MFA and conditional-access requirements, prohibit session sharing across unrelated users, and revoke access when work ends. Do not put cookies, tokens, or passwords in prompts, Git state, Hub pages or screenshots.

## Current authentication boundary

The reviewed HTTP router and capability dispatch do not establish a native authenticated end-user principal or per-user authorization policy. Provider authentication and host secret storage are separate facilities. Do not expose the daemon to untrusted clients on the assumption that provider login protects it.

An enterprise gateway must cover every route and make the backend inaccessible by alternate paths. A shared worker identity can still collapse several humans into one downstream actor. If individual attribution or separation of duties is required, add a verified identity propagation and authorization design or use isolated workers per principal.

## Acceptance tests

- Unauthenticated, expired and revoked sessions cannot reach any control route or live stream; active streams terminate within the agreed revocation window.
- A lower-privilege user cannot launch a higher-privilege action or use another user's session.
- Alternate hostnames, direct ports and proxy header spoofing do not bypass entry controls; cross-origin mutation requests are rejected by the deployed protection layer.
- A representative action connects the initiating human, run/Goal, worker, target identity, time, outcome and approval record without logging secrets.
- The host can stop execution and revoke downstream access independently of agent cooperation.

## Existing browser-request protection

Refine checks Origin/Referer against the request host on non-GET HTTP requests. The helper permits requests with neither header; it does not establish caller identity. Preserve this protection through proxy configuration and test it alongside authentication. See the [source trace](01-how-it-works.md#implementation-details-that-affect-the-controls), [operating plan](06-operating-plan.md) and [verification workbook](09-verification.md).
