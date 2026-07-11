# Import

## Key Ideas

- **Work Creation At Scale**: import should turn external lists, plans, transcripts, and files into structured Goals and Features.
- **Review Before Persist**: AI extraction should produce drafts users can inspect before saving.
- **Shared Persistence**: imported work should enter the same Goal and Feature model as manually created work.
- **Deduplication**: import should help avoid duplicate or overlapping work.
- **Bridge From Unstructured To Structured**: import converts messy source material into actionable work.
- **Source-Sensitive Extraction**: feature specs and Plan transcripts can be decomposed more deeply than simple lists.

## Purpose

The Import surface exists because work often starts outside Refine: spreadsheets, CSVs, planning notes, chat transcripts, bug lists, audits, and AI-generated plans.

This surface exposes the shared Import capability. It should make extraction, draft review, deduplication, and persistence usable in the browser without turning import into browser-only behavior.

Import should make that material useful without forcing users to manually create every Goal. It should preserve review and correction before durable state changes.

## Expected Role

Import should support CSV parsing, AI extraction, draft review, deduplication, Feature assignment, standalone Goal creation, and persistence through shared work item services. It should use architecture-aware extraction for Plan/spec-like input, while keeping CSV rows, issue lists, bug lists, and short notes direct.

Current implementation details that matter to intent:

- import flows are exposed through the Goals navigation and toolbar-related drafting flows;
- draft state and draft tables are separate UI concerns;
- feature-spec extraction may create one Feature plus dependency-aware Goal drafts;
- background extraction should be visible through operations and System notices;
- final persistence should use shared Goal and Feature creation behavior.

Import should not create a parallel work format. Its successful output should be ordinary Refine work.

## Future Direction

Future import should become a major agentic planning bridge. Agents may read long documents, code audits, issue trackers, chats, and design specs, then propose Features and Goals with evidence and dependencies.

The direction is not blind bulk creation. The direction is AI-assisted structuring with reviewable drafts and durable intent.
