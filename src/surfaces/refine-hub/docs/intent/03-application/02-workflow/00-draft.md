# Draft

Draft captures ideas and personal tasks before acceptance into Backlog. A Draft is an ordinary Goal, not a separate Goal type. It can have a name, description, Reporter, priority, notes and a Project Planning card without an execution Round.

Draft Goals are excluded from workflow scheduling and do not block Feature order or priority queues. Creation does not create an implementation branch or checkout. Optional configured lifecycle Skills still run through managed Skill admission and may use a separate lifecycle checkout.

Accepting a Draft uses the shared transition into Backlog and its lifecycle gates. A release lane then proceeds through Backlog and Todo on the selected execution node. A lane name alone never changes workflow status. Existing direct Goal creation keeps Backlog as its default; callers can explicitly select Draft.

See [Project Planning](../06-project-planning.md) for boards, routing, lane Skills, commands and migration.
