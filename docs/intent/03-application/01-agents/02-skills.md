# Skills

## Key Ideas

- **Instructions Are Plain Text**: users describe how an agent should work without authoring transport envelopes or output schemas.
- **One Trigger per Skill**: choose one workflow or lifecycle point, or Custom for manual execution. Clone a Skill to use its instructions elsewhere.
- **Triggers Determine Results**: the system supplies the completion contract for the selected trigger. Users do not configure result roles.
- **Context Is Parameterized**: instructions receive explicit Event input, Goal context, System context, and declared defaults.
- **Evidence Remains Authoritative**: an agent's confidence cannot substitute for a supervised check or an exact candidate proof.

## Purpose

Skills let projects and nodes express repeatable agent behavior in one place. They replace the separate Governance, Quality, and Guidance editors and fixed planning launch sequence while preserving Refine's workflow, recovery, and integration responsibilities.

## Expected Role

A Skill contains a stable ID, name, plain-text instructions, enabled state, scope, parameters, and one trigger. Enter and Exit triggers cover all ten workflow steps; Node starts covers lifecycle startup; Custom makes a Skill manually runnable. Refine supplies the completion contract and pins invocation identity, binding identity, and role. Structured completion reports distinguish success, a failed finding, and an execution error. Invalid responses retain their evidence and receive at most two diagnostic repairs.

Refine seeds Plan, Implement, Quality, and Governance Skills with blocking bindings at their corresponding Workflow Enter Events. Projects can edit those prompts or configure multiple Skills at the same trigger. Required automated phases need an enabled required Skill at their Enter trigger. Plan Skills produce independent final plans; Implement receives the collected checklists and reports evidence for their namespaced IDs. Quality proposes observable checks that Refine supervises against the exact candidate. Governance reads the candidate and reports actual violations with actionable recovery requests. Plan and Governance remain observational.

There is one default Plan Skill. The prompt can choose how to investigate, criticize, and finalize its work; Refine does not impose a separate proposal, criticism, and revision agent sequence. Existing finalized plans and historical planning artifacts remain readable. Scoped finding recovery invokes the Plan Skill with the reviewed recovery request and retained candidate while preserving later gates.

Context attachment bindings reuse Skill text without spawning an agent. Imported Guidance becomes context Skills with its applicability and enabled state preserved. Existing Governance and Quality instructions become default Skill text. Migration archives the originals before installing the new configuration and is safe to resume. Previously enforced Quality commands remain supervised until the imported Quality Skill is deliberately edited or replaced. Legacy files do not regain configuration authority after installation.

The Skill editor has one Trigger field. Workflow options and parameter context are optional, collapsed details. A Skill and its trigger save atomically under an observed revision; concurrent edits fail without erasing the current configuration. Deleting a Skill removes its trigger. Editing instructions never rewrites completed invocation or Round evidence. Clone Skill creates an independent copy with a new identity.

Controls → Skills and the command palette list enabled Custom Skills for the selected node. Manual runs launch only the selected Skill, independently of an open Goal. The browser collects typed inputs and opens an agent tab using the existing managed terminal lifecycle, transcript, reconnect, and stop controls. The CLI launches a supervised task and exposes its results, history, and cancellation through `refine skills`.

Configurations from the earlier multiple-trigger model are archived and converted deterministically: each assignment receives its own Skill while preserving instructions, parameters, execution order, effective scope, enabled state, and existing override relationships. Historical runs retain their pinned snapshots.

## Future Direction

Improve reuse, discovery, and context selection while keeping authored instructions readable. Additional result contracts should remain Refine-owned so richer execution does not turn Skill editing into schema programming.
