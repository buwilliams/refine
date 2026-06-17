# Import

## Key Ideas

- **Work Creation At Scale**: import should turn external lists, plans, transcripts, and files into structured Gaps and Features.
- **Review Before Persist**: AI extraction should produce drafts users can inspect before saving.
- **Shared Persistence**: imported work should enter the same Gap and Feature model as manually created work.
- **Deduplication**: import should help avoid duplicate or overlapping work.
- **Bridge From Unstructured To Structured**: import converts messy source material into actionable work.

## Purpose

The Import surface exists because work often starts outside Refine: spreadsheets, CSVs, planning notes, chat transcripts, bug lists, audits, and AI-generated plans.

Import should make that material useful without forcing users to manually create every Gap. It should preserve review and correction before durable state changes.

## Expected Role

Import should support CSV parsing, AI extraction, draft review, deduplication, Feature assignment, standalone Gap creation, and persistence through shared work item services.

Current implementation details that matter to intent:

- import flows are exposed through the Gaps navigation and toolbar-related drafting flows;
- draft state and draft tables are separate UI concerns;
- background extraction should be visible through operations and System notices;
- final persistence should use shared Gap and Feature creation behavior.

Import should not create a parallel work format. Its successful output should be ordinary Refine work.

## Future Direction

Future import should become a major agentic planning bridge. Agents may read long documents, code audits, issue trackers, chats, and design specs, then propose Features and Gaps with evidence and dependencies.

The direction is not blind bulk creation. The direction is AI-assisted structuring with reviewable drafts and durable intent.
