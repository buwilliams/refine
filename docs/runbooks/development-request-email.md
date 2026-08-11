# Operate the development-request email intake

Use this runbook only for a local Refine installation that develops Refine
itself. The example connects `~/projects/refine/run/8082` to the Fastmail
address `goal@getrefine.dev` and pins intake to `~/projects/refine-next`.
Fastmail is the durable queue: mail continues to arrive while Refine is stopped,
and the daemon processes it after restart.

This is deliberately not a project setting. A production Refine installation
without the port-local capability file does not launch an email worker, access
Fastmail, or create an email-request ledger.

## Preconditions

- `getrefine.dev` is active as a Fastmail custom domain while Cloudflare remains
  its authoritative DNS provider.
- A test message reaches `goal@getrefine.dev` in Fastmail.
- Refine is attached to the intended target repository.
- The configured agent provider is installed and authenticated locally.

Do not enable Cloudflare Email Routing for this domain after its MX records
point to Fastmail. Keep the existing website records in Cloudflare unchanged.

## Finish the Fastmail address setup

1. Confirm `goal@getrefine.dev` is an address or alias on the account and is
   available as a sending identity.
2. Under **Settings -> Privacy & Security -> Manage API tokens**, create a token
   for Refine with mail read/write and submission access. Copy it once.

No dedicated mailbox or Fastmail filing rule is required. Refine queries mail
addressed to `goal@getrefine.dev` wherever Fastmail filed it, then accepts only
senders present in the host-local allowlist.

## Store the token locally

With Refine running on port 8082, store the Fastmail token in the native Refine
secret store. Substitute the token without putting it in shell history when
that matters on the host:

```bash
curl --fail-with-body \
  -X PUT http://127.0.0.1:8082/api/agents/secrets/email/fastmail_jmap_token \
  -H 'content-type: application/json' \
  --data '{"value":"PASTE_FASTMAIL_TOKEN"}'
```

The token must not be copied into project settings or committed files.

## Install the local capability contract

Create `~/projects/refine/run/8082/self-development-email.json` on the Refine
host. The file belongs to the running Refine installation, not to its target
repository, and must not be committed or synchronized:

```json
{
  "schema_version": 1,
  "target_root": "/home/buddy/projects/refine-next",
  "address": "goal@getrefine.dev",
  "allowed_senders": [
    "person@example.com"
  ],
  "poll_seconds": 60,
  "auto_approve_after_seconds": 0
}
```

The configured `target_root` must be absolute and resolvable. Refine compares
its canonical path with the currently active target before it reads the token
or contacts Fastmail. Switching this installation to any other target therefore
leaves incoming mail queued at Fastmail.

Sender matching is case-insensitive. A zero auto-approve delay means the next
poll approves an email-linked Goal as soon as it reaches Review; approval still
uses Refine's candidate-integration and publication checks before moving it to
Done. An optional non-empty `agent_cli` overrides the target's ordinary
`agent_cli`; when omitted, the target setting is inherited.

Edit `allowed_senders` in this file to change the list. The runner rereads and
validates the complete file each polling cycle.

## Processing contract

For each accepted Fastmail message, Refine:

1. queries mail addressed to the configured recipient and checks the local
   sender allowlist;
2. persists a retry record before marking the Fastmail message processed;
3. supplies the plain body and `.txt` attachments to one review agent, ignoring
   images and every other attachment type;
4. creates at most one deterministic, low-priority Goal in Backlog;
5. lets the normal backlog and workflow automation run;
6. approves the Goal from Review only after verified Ready Merge evidence; and
7. sends a threaded resolution reply from `goal@getrefine.dev` after Done.

Request records live only below the installation runtime at
`run/8082/self-development-email/requests/<request-id>/request.json`. A
deterministic outbound Message-ID plus a Sent-mail lookup prevents a restart
between send and local settlement from sending the same resolution twice.

The runner is owned by the local daemon. Stopping Refine stops polling, agent
review, Goal creation, approval, and replies; Fastmail continues queuing mail.

## Verify end to end

1. Send a small request from one allowlisted address.
2. Confirm a new `DR...` Goal appears in Backlog within the polling interval.
3. Confirm generated image or non-text attachments do not appear in the Goal
   prompt, while a `.txt` attachment does.
4. Let the Goal reach Review and confirm it advances to Done through approval.
5. Confirm the sender receives one threaded resolution reply.
6. Stop Refine, send another request, wait longer than one polling interval,
   and confirm no Goal appears until Refine is started again.

If intake fails, inspect the request record's `last_error` and the daemon's
`refine development requests:` log line. Common causes are a missing token, a
sender absent from the allowlist, or an API token without mail/submission
access.

## Disable or rotate

Disable processing without changing Fastmail by removing or renaming
`run/8082/self-development-email.json`. The worker exits after its next polling
cycle and the daemon does not relaunch it while the file is absent.

Queued Fastmail messages remain available. To rotate the token, create the new
Fastmail token, overwrite `email/fastmail_jmap_token` through the same PUT
route, verify one poll, then revoke the old token in Fastmail.
