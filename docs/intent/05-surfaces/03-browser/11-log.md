# Logs

The Toolbar is the browser's single log reader. Logs have no main navigation screen. Legacy Logs links open the corresponding Toolbar tab. A visible View Logs button on the Goal modal opens that Goal's log tab.

System receives normal operational events by default. An explicit Start tail / Stop tail control enables or stops following all retained application activity, Round logs, and raw agent/process stdout and stderr. Collection is off by default for System, including after reload or project switching. Stopping tail never stops agents, processes, or ordinary system notices.

Goal log tabs follow the selected Goal's complete retained history across Rounds and managed processes. Both viewers use a flat chronological stream with inline details, one scrollbar, and no per-entry expansion controls. Following does not pull the reader away when they scroll back. Older pages stop following until the user starts it again.

Search runs against retained records, including messages, structured details, and raw output; it is not limited to the browser's recent buffer or the bounded activity projection. Type (application event, Round log, operation log, API activity, stdout, stderr) is independent of severity. Category, actor, and process filters combine with text search. Raw stderr is not automatically treated as an application error. Counts and pagination make older matches reachable. Source identity remains visible; raw output without recorded line timestamps identifies its time as process start or receipt time rather than claiming an exact event time.

Live tails use byte cursors and bounded pages. Repeated reads do not duplicate output or skip entries when a page fills. Filters affect presentation and search, not what the system records. Node/project switches and stale search responses must never mix scopes. Retained logs remain evidence; UI changes do not delete their records.
