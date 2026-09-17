# Project Planning

Open **Project Planning** from Main to organize shared boards, lanes and cards. Every node attached to the project sees the same boards after state synchronization. Choosing a Reporter changes attribution for new work; it does not hide other people's cards.

Create a board for a project or for your own todo list. Add lanes, then add cards or attach existing Goals. Each new card is an ordinary Goal in **Draft**. Give it a name and description, set its Reporter, and keep notes through its Goal page. Use drag-and-drop or the **Move** button to organize cards. Keyboard users can focus a card and press **M** to open Move. Archive completed personal tasks or move them to a Done lane.

Lane names organize the board. Goal badges show actual workflow progress. Moving a personal task into Done does not mark its Goal as delivered.

## Release ready work

In lane settings, choose one action:

- **Organize only** keeps work in its current workflow step.
- **Accept into Backlog** accepts a Draft without starting execution.
- **Release to execution** accepts the Draft, creates its first execution Round when needed, and releases it through Todo on the selected node.

Set execution routing on the board, lane or individual card. A card overrides its lane, and a lane overrides its board. Automatic routing selects an eligible node with the least active work. Required lane and workflow Skills run before work moves forward. A card needs a Reporter before its first release.

Changing settings does not release cards already in a lane. Use **Apply lane action** when you intend to apply the new settings. Already executing Goals retain their current workflow; organizing their cards does not restart them.

## Follow pending actions

Moves and releases run on the Goal's current node. The board shows pending work and its reason. An offline owner needs to return or the Goal needs an explicit transfer. Cross-node releases wait for shared state to synchronize.

**Details** shows the processor, execution target and Skill invocation IDs. **Cancel** stops further processing and retains completed changes. If routing or card content needs correcting while an action waits, cancel it, make the correction, and apply the lane action again. A failed Skill remains evidence; inspect it before requesting another action.

## Import existing Todo Lists

Upgrade every node sharing the project, then choose **Import Todo Lists** once from the intended owning node. Import creates one board per old list, with Open and Done lanes. Existing completion marks, text, Reporter and timestamps are retained. Every imported item starts as a Draft Goal, including completed personal tasks. Import does not launch Skills or implementation.

The original Todo data is retained for recovery. After migration, old Todo interfaces reject writes. See the [technical contract and CLI examples](../docs/intent/03-application/06-project-planning.md) for automation and upgrade details.
