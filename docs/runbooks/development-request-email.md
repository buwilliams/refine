# Fetch Goals from Email with a Skill

Use the **Fetch Goals from Email** Custom Skill to import requests addressed to
`goal@getrefine.dev` into the selected project's Backlog. Fastmail keeps mail
queued while Refine is stopped. The Skill performs bounded fetches when invoked;
there is no dedicated polling worker, automatic acceptance, or resolution reply.

## Configure the local connection

Keep the existing Fastmail domain and address configuration. The mail token
needs read/write access; fetching does not require submission access. Store it
through the native secret API at
`PUT /api/agents/secrets/email/fastmail_jmap_token` with a JSON `value` field.
Never include the token in a Skill, command output, or synchronized state.

The host-local file `run/8082/self-development-email.json` remains the connection
and authorization boundary. Existing files work unchanged; polling and approval
fields are ignored. A minimal connection is:

```json
{
  "schema_version": 1,
  "target_root": "/home/buddy/projects/refine-next",
  "address": "goal@getrefine.dev",
  "allowed_senders": ["person@example.com"]
}
```

The canonical target must match before Refine accesses the token or the request
ledger. Sender matching is case-insensitive. This local connection and its
secret are not synchronized through refine-state.

## Install and run the Skills

Save [Fetch Goals from Email](skills/fetch-goals-from-email.json) through
**Settings → Skills** or `refine skills save`, using the latest configuration
revision. Choose the node that owns the mailbox connection. Open
**Controls → Skills → Fetch Goals from Email** to run it in an agent tab.

The Skill uses the supported one-shot command:

```sh
refine system fetch-email-goals \
  --runtime-root /home/buddy/projects/refine/run/8082 \
  --target-root /home/buddy/projects/refine-next
```

Each call fetches at most 25 remote messages. Its JSON result includes
`fetched_count`, `batch_limit`, `goal_ids`, and per-record `errors`. Errors cause
a nonzero exit while retaining successful imports and retry evidence. The Skill
may fetch further batches, up to ten per invocation, and reports remaining work.

For startup fetching, also install
[Fetch Goals from Email on startup](skills/fetch-goals-from-email-on-startup.json)
on that node. Set its daemon port parameter to the installation's port. Its
**Node starts** trigger queues the Custom fetch Skill with an occurrence-specific
request ID, then exits. It never waits for the child agent while holding an
execution slot. Each Skill retains one trigger, and the fetch instructions remain
in one place. Mail arriving later waits until the next manual run or startup.

## Reliability and evidence

Concurrent fetch calls serialize before remote acknowledgement and Goal
creation. Each accepted message is durably recorded before it receives the
processed keyword in Fastmail. At most one deterministic low-priority Goal is
created, containing the sender, subject, decoded body, and named text attachments
in MIME order. Images, binary bodies, and unnamed attachments are excluded.
The sender remains the Reporter and normal default assignee behavior applies.

Existing request records remain under
`run/8082/self-development-email/requests/<request-id>/request.json`. Received
schema-1 records still require their original raw message before source
migration and authoring. Invalid records remain unchanged and are reported
without blocking later valid records. Linked and terminal records are retained
without modifying historical Goals or sending messages.

Normal workflow behavior determines when imported Goals run. Review acceptance
uses the ordinary manual approval and follow-up Round controls. The retired
email-specific Auto-approve setting is ignored and cannot be configured.

## Verify and disable

Run the Skill and confirm its agent output identifies the imported Goals or
reports no new mail. Repeating a fetch must not duplicate a Goal. Restart Refine
and inspect Skill execution history for one startup invocation and its queued
Custom fetch invocation. Check real completion evidence, not just the queued
receipt.

Disable the startup Skill to stop automatic fetching. Disable the Custom Skill
to remove manual availability. Removing the local connection prevents the fetch
capability from accessing mail; queued messages and historical records remain.
Rotate the token through the same secret API without copying it into Skills.
