# Nav

## Key Ideas

- **Orientation First**: nav should tell the user where they are and which app/node context is active.
- **Primary Work Paths**: Dashboard, Features, Goals, Changes, and Logs are first-class routes.
- **Context Controls**: app status, reporter, agent status, command palette, and create actions belong in the shell.
- **Stable Entry Points**: nav should be predictable enough for repeated daily use and future agent-driven UI control.

## Purpose

Navigation exists to make Refine's operating context immediately visible and to move users to the main work surfaces without ceremony.

The topbar is not just a list of pages. It shows the active node, active app context, reporter context, target-app status, agent status, command palette access, Guide access, management links, and primary create actions.

## Expected Role

Nav should keep the system grounded. If the user is attached to the wrong app, using the wrong reporter, or agents are active, the shell should make that context visible before the user takes action.

The current browser shell uses Dashboard, Features, Goals, Changes, and Logs as the main nav items. Management actions live in context menus so the main nav stays focused on work. The bright primary create action is `+ New Goal`, with related creation flows available nearby.

Nav should not hide important operating state in deep settings pages. Active app, node, target-app status, and agent status are part of the user's working context.

## Future Direction

Future navigation may become more command-palette and agent-driven. As agents take over more work, nav should help people jump to exceptions, evidence, pending review, active processes, and high-risk changes.

The nav should remain quiet and utilitarian: fewer marketing surfaces, more direct access to the work and system state that matter.
