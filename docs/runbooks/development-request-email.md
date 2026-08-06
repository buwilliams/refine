# Operate the development-request email intake

Use this runbook to connect the Fastmail mailbox `goal@getrefine.dev` to the
currently active Refine project. Fastmail is the durable queue: mail continues
to arrive while Refine is stopped, and the daemon processes it after restart.

## Preconditions

- `getrefine.dev` is active as a Fastmail custom domain while Cloudflare remains
  its authoritative DNS provider.
- A test message reaches `goal@getrefine.dev` in Fastmail.
- Refine is attached to the intended target repository.
- The configured agent provider is installed and authenticated locally.

Do not enable Cloudflare Email Routing for this domain after its MX records
point to Fastmail. Keep the existing website records in Cloudflare unchanged.

## Finish the Fastmail mailbox setup

1. In Fastmail, create a mailbox named `Development Requests`.
2. Confirm `goal@getrefine.dev` is an address or alias on the account and is
   available as a sending identity.
3. Add a Fastmail rule whose condition matches mail addressed to
   `goal@getrefine.dev` and whose action moves it to `Development Requests`.
4. Under **Settings -> Privacy & Security -> Manage API tokens**, create a token
   for Refine with mail read/write and submission access. Copy it once.

The folder rule is important: Refine intentionally polls only this dedicated
mailbox, not the account-wide inbox.

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

## Enable the active project

Apply the project-local settings:

```bash
curl --fail-with-body \
  -X PATCH http://127.0.0.1:8082/api/settings \
  -H 'content-type: application/json' \
  --data @- <<'JSON'
{
  "development_request_email_enabled": true,
  "development_request_address": "goal@getrefine.dev",
  "development_request_mailbox": "Development Requests",
  "development_request_allowed_senders": "bwilliams@nevo.com, ejacobson@nevo.com, Ethan.Jacobson@insurity.com, Buddy.Williams@insurity.com, buddywilliams@gmail.com",
  "development_request_poll_seconds": 60,
  "development_request_auto_approve_after_seconds": 0,
  "development_request_agent_cli": ""
}
JSON
```

An empty `development_request_agent_cli` inherits `agent_cli`. Sender matching
is case-insensitive. A zero auto-approve delay means the next poll approves an
email-linked Goal as soon as it reaches Review; approval still uses Refine's
candidate-integration and publication checks before moving it to Done.

The allowlist is ordinary project-local Refine configuration and can change
without rebuilding. A PATCH replaces the full list on the next polling cycle:

```bash
curl --fail-with-body \
  -X PATCH http://127.0.0.1:8082/api/settings \
  -H 'content-type: application/json' \
  --data '{"development_request_allowed_senders":"first@example.com, second@example.com"}'
```

## Processing contract

For each accepted Fastmail message, Refine:

1. checks the local sender allowlist;
2. persists a retry record before marking the Fastmail message processed;
3. supplies the plain body and `.txt` attachments to one review agent, ignoring
   images and every other attachment type;
4. creates at most one deterministic Goal in Backlog;
5. lets the normal backlog and workflow automation run;
6. approves the Goal from Review only after verified Ready Merge evidence; and
7. sends a threaded resolution reply from `goal@getrefine.dev` after Done.

Request records live below the project's Refine state directory at
`development-requests/<request-id>/request.json`. A deterministic outbound
Message-ID plus a Sent-mail lookup prevents a restart between send and local
settlement from sending the same resolution twice.

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
`refine development requests:` log line. Common causes are a missing token,
wrong mailbox name, a sender absent from the allowlist, or an API token without
mail/submission access.

## Disable or rotate

Disable processing without changing Fastmail:

```bash
curl --fail-with-body \
  -X PATCH http://127.0.0.1:8082/api/settings \
  -H 'content-type: application/json' \
  --data '{"development_request_email_enabled":false}'
```

Queued Fastmail messages remain available. To rotate the token, create the new
Fastmail token, overwrite `email/fastmail_jmap_token` through the same PUT
route, verify one poll, then revoke the old token in Fastmail.
