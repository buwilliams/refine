# Files

## Key Ideas

- **Source Proximity**: users should inspect project files without leaving Refine.
- **Searchable Context**: file search helps users and agents find relevant code quickly.
- **Read-Oriented By Default**: file browsing should support inspection before mutation.
- **Toolbar Utility**: files belong near chat and terminal because they support active work.

## Purpose

The Files surface exists to keep code context close to work context. When reviewing a Goal, discussing with an agent, or diagnosing a change, users often need to inspect source files.

Files should help users and agents connect Refine work back to the actual project.

## Expected Role

Files should provide a lightweight tree, path selection, file reading, chunked content, and search. It should integrate with command palette and toolbar flows so users can jump to a file quickly.

Current implementation details that matter to intent:

- Files is a standard toolbar tab;
- the file tree is depth and entry limited for performance;
- file reads use chunking for large files;
- file search has debounce, selected-result state, and Enter-to-open behavior;
- command palette can open Files search directly.

Files should not become a full IDE. It should provide enough source visibility to support Refine work and agent collaboration.

## Future Direction

Future Files behavior may support semantic search, agent-generated context bundles, diff-aware navigation, and evidence links from Goals to source locations.

The goal is source context that improves work quality without turning Refine into a heavyweight editor.
