# Import

## Key Ideas

- **Work Creation At Scale**: import should turn external lists, plans, transcripts, and files into structured Goals and Features.
- **Bridge From Unstructured To Structured**: import converts messy source material into actionable work.
- **Review Before Persist**: AI extraction should produce drafts users can inspect before saving.
- **Shared Work Model**: imported work should enter the same Goal and Feature model as manually created work.
- **Deduplication And Evidence**: import should help avoid duplicate work and preserve source context where possible.
- **Planning When Appropriate**: long specs and Plan transcripts may use architecture-aware extraction, but simple imports should stay direct.

## Purpose

Import exists because work often starts outside Refine: spreadsheets, CSVs, planning notes, chat transcripts, bug lists, audits, issue trackers, code reviews, and AI-generated plans.

The capability should make that material useful without forcing users to manually create every Goal. It should transform external material into reviewable draft work, then persist accepted drafts as ordinary Refine Goals and Features.

Import is a capability, not just a browser flow. A future agent, CLI command, API route, or desktop surface should be able to use the same extraction, draft, deduplication, and persistence behavior.

## Expected Role

Import should sit between raw source material and durable work state. It should support:

- CSV and structured-file parsing;
- AI-assisted extraction from unstructured text;
- draft review before durable state changes;
- deduplication against existing Goals and Features;
- Feature assignment or Feature creation when source material describes larger outcomes;
- source evidence that explains where imported work came from;
- persistence through shared work item services.

Import should not create a parallel work format. Its successful output should be ordinary Refine work: Goals, Features, notes, ordering, source context, and evidence that workflow and agents can use.

AI Import should adapt to the source material. A long product spec or Plan transcript can be decomposed with architecture lenses such as persistence, logic, surfaces, integrations, recovery, and tests. A CSV, bug list, issue list, or short note should be imported directly into draft work without forcing those lenses.

Current implementation details that matter to intent:

- import flows are exposed through browser work surfaces;
- draft state and draft tables are review concerns before persistence;
- Plan/spec-like extraction uses broader planning guidance before producing ordinary drafts;
- CSV, issue-list, and simple AI extraction remain direct draft creation paths;
- background extraction should be visible through operations and System notices;
- final persistence should use shared Goal and Feature creation behavior.

## Future Direction

Future import should become a major agentic planning bridge. Agents may read long documents, code audits, issue trackers, chats, design specs, repository history, and product feedback, then propose Features and Goals with evidence and dependencies.

The direction is not blind bulk creation. The direction is AI-assisted structuring with reviewable drafts, deduplication, source evidence, and durable intent.
