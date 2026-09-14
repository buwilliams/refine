# Customize agent prompts

Open **Settings → Prompts** to see which instructions Refine sends to agents.

## Skills describe the work

A Skill explains a task, its desired outcome, and its guardrails. Assign an existing Skill to a Goal step or system event, or create a new one. A Skill can be used by several events, and an event can run several Skills.

Goal-step tabs show how many Skills are assigned on entry, success, error, and exit. Open a Skill card to edit it or preview its prompt. Custom actions also have a **Run Skill** button.

## Templates assemble the context

Shared resources include agent Templates and smaller reusable pieces of context. Variables such as `{{skill}}`, `{{refine_executable}}`, and `{{current_round_goal}}` insert the relevant content. Included Templates can contain variables too.

The **How agent prompts are built** map shows the Templates used by the selected agent type. Its launch-path selector distinguishes toolbar Terminal sessions from Managed chat sessions used through the API. It explains existing paths; changing this selection does not launch or configure an agent.

## Recover a default

Reset an individual Template from the resource list, or reset all Templates used by the selected agent type. Review the confirmation: some Templates are shared, so restoring them can affect other agent types too. These resets leave Skills unchanged.

## See the actual prompt

Open a Goal's **Prompts** tab and select a recorded launch. It displays the full retained Refine prompt, including expanded Template content. Provider instructions and subsequent conversation messages are separate. If an older launch did not retain its prompt, Refine says so.

Use this view to identify missing context or repeated instructions before editing Templates.
