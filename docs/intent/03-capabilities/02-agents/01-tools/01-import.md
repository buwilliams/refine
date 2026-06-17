# Import

## Key Ideas

- **Work Creation At Scale**: import should turn external lists, plans, transcripts, and files into structured Gaps and Features.
- **Bridge From Unstructured To Structured**: import converts messy source material into actionable work.
- **Review Before Persist**: AI extraction should produce drafts users can inspect before saving.
- **Shared Work Model**: imported work should enter the same Gap and Feature model as manually created work.
- **Deduplication And Evidence**: import should help avoid duplicate work and preserve source context where possible.

## Purpose

Import exists because work often starts outside Refine: spreadsheets, CSVs, planning notes, chat transcripts, bug lists, audits, issue trackers, code reviews, and AI-generated plans.

The capability should make that material useful without forcing users to manually create every Gap. It should transform external material into reviewable draft work, then persist accepted drafts as ordinary Refine Gaps and Features.

Import is a capability, not just a browser flow. A future agent, CLI command, API route, or desktop surface should be able to use the same extraction, draft, deduplication, and persistence behavior.

## Expected Role

Import should sit between raw source material and durable work state. It should support:

- CSV and structured-file parsing;
- AI-assisted extraction from unstructured text;
- draft review before durable state changes;
- deduplication against existing Gaps and Features;
- Feature assignment or Feature creation when source material describes larger outcomes;
- source evidence that explains where imported work came from;
- persistence through shared work item services.

Import should not create a parallel work format. Its successful output should be ordinary Refine work: Gaps, Features, notes, ordering, source context, and evidence that workflow and agents can use.

Current implementation details that matter to intent:

- import flows are exposed through browser work surfaces;
- draft state and draft tables are review concerns before persistence;
- background extraction should be visible through operations and System notices;
- final persistence should use shared Gap and Feature creation behavior.

## Future Direction

Future import should become a major agentic planning bridge. Agents may read long documents, code audits, issue trackers, chats, design specs, repository history, and product feedback, then propose Features and Gaps with evidence and dependencies.

The direction is not blind bulk creation. The direction is AI-assisted structuring with reviewable drafts, deduplication, source evidence, and durable intent.
