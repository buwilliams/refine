# Project Planning

Open **Planning** below Tools in the navigation to organize shared boards, lanes and cards. Every node attached to the project sees the same boards after state synchronization. Choosing a Reporter changes attribution for new work; it does not hide other people's cards.

Use the Planning dropdown to create or select a board for a project or your own todo list. Add lanes, then choose **Add card**. Type an idea and press **Enter** to create it, or select an existing Goal from the search results. Use **↑ / ↓** to choose a result and **Escape** to close the composer. Each new card is an ordinary Goal in **Draft**, visible only in Project Planning until it enters Backlog. Edit its title, description, Reporter, and priority on the board. Use drag-and-drop or the **Move** button to organize cards. Keyboard users can focus a card and press **M** to open Move. Archive completed personal tasks or move them to a Done lane.

The **Show archived** toggle reveals archived boards and cards. Drag cards between lanes or above and below other cards; the insertion marker shows their destination.

Lane names organize the board. Goal badges show actual workflow progress. Moving a personal task into Done does not mark its Goal as delivered.

## Release ready work

In lane settings, choose one action:

- **Organize only** keeps work in its current workflow step.
- **Accept into Backlog** accepts a Draft without starting execution.
- **Release to execution** accepts the Draft, creates its first execution Round when needed, and releases it through Todo on the selected node.

Set execution routing on the board, lane or individual card. A card overrides its lane, and a lane overrides its board. Automatic routing selects an eligible node with the least active work. Required lane and workflow Skills run before work moves forward. A card needs a Reporter before its first release.

Changing settings does not release cards already in a lane. Use **Send to Backlog** or **Release to execution** on a card when you intend to apply the new settings. Already executing Goals retain their current workflow; organizing their cards does not restart them.

## Follow pending actions

Moves and releases run on the Goal's current node. The board shows pending work and its reason. An offline owner needs to return or the Goal needs an explicit transfer. Cross-node releases wait for shared state to synchronize.

**Details** shows the processor, execution target and Skill invocation IDs. **Cancel** stops further processing and retains completed changes. If routing or card content needs correcting while an action waits, cancel it, make the correction, and apply the lane action again. A failed Skill remains evidence; inspect it before requesting another action.

## Import existing Todo Lists

Upgrade every node sharing the project, then choose **Import Todo Lists** once from the intended owning node. Import creates one board per old list, with Open and Done lanes. Existing completion marks, text, Reporter and timestamps are retained. Every imported item starts as a Draft Goal, including completed personal tasks. Import does not launch Skills or implementation.

The original Todo data is retained for recovery. After migration, old Todo interfaces reject writes. See the [technical contract and CLI examples](../docs/intent/03-application/06-project-planning.md) for automation and upgrade details.

## Close or delete a board

Open boards appear below Planning in the navigation. Use the close button on a board row to close its view. The board and cards remain shared, and you can reopen it from the Planning dropdown.

To remove a board, open **Board settings → Delete board** and confirm. This removes the board and its card placements while retaining the underlying Goals. Retained Goals can be added to another board through Add card search. Finish or cancel pending board actions before deleting. Deletion is shared across nodes; closing a view is local to your browser.
