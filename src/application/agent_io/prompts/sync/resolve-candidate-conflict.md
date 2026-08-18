Rebasing this Goal's candidate left conflicts in this workspace.

Goal prompt:

{{goal_prompt}}

Round intent:

{{round_intent}}

Other goals' implementation reports behind the conflicting commits:

{{other_goal_reports}}

Conflicted files:

{{conflicted_files}}

Resolve every conflict marker so that both intents survive together. Make no other edits: a change to any file outside the conflicted list is rejected.

If both intents cannot survive together, edit nothing and reply with a line beginning `NEEDS DECISION:` and your question for an operator, naming the Goal, the file, and the choice.
