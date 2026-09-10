# Skills

## Key Ideas

- **Instructions Are Plain Text**: users describe how an agent should work without authoring transport envelopes or output schemas.
- **Events Trigger Skills**: a reusable instruction is connected to activity through an explicit Event binding.
- **Roles Describe Results**: the role tells Refine how to interpret completion; it does not choose a separate orchestration engine.
- **Context Is Parameterized**: instructions receive explicit Event input, Goal context, System context, and declared defaults.
- **Evidence Remains Authoritative**: an agent's confidence cannot substitute for a supervised check or an exact candidate proof.

## Purpose

Skills let projects and nodes express repeatable agent behavior in one place. They replace the separate Governance, Quality, and Guidance editors and fixed planning launch sequence while preserving Refine's workflow, recovery, and integration responsibilities.

## Expected Role

A Skill contains a stable ID, name, plain-text instructions, enabled state, scope, parameters, and a result role. Roles cover the ten workflow steps and general Task work. Refine supplies the completion contract and pins invocation identity, binding identity, and role. Structured completion reports distinguish success, a failed finding, and an execution error. Invalid responses retain their evidence and receive at most two diagnostic repairs.

Refine seeds Plan, Implement, Quality, and Governance Skills with blocking bindings at their corresponding Workflow Enter Events. Projects can edit those prompts or bind multiple Skills of the same role. Required automated phases need an enabled blocking Skill of their matching role. Plan Skills produce independent final plans; Implement receives the collected checklists and reports evidence for their namespaced IDs. Quality proposes observable checks that Refine supervises against the exact candidate. Governance reads the candidate and reports actual violations with actionable recovery requests. Plan and Governance remain observational.

There is one default Plan Skill. The prompt can choose how to investigate, criticize, and finalize its work; Refine does not impose a separate proposal, criticism, and revision agent sequence. Existing finalized plans and historical planning artifacts remain readable. Scoped finding recovery invokes the Plan Skill with the reviewed recovery request and retained candidate while preserving later gates.

Context attachment bindings reuse Skill text without spawning an agent. Imported Guidance becomes context Skills with its applicability and enabled state preserved. Existing Governance and Quality instructions become default Skill text. Migration archives the originals before installing the new configuration and is safe to resume. Previously enforced Quality commands remain supervised until the imported Quality Skill is deliberately edited or replaced. Legacy files do not regain configuration authority after installation.

The Skill editor selects the system and custom Events that trigger it. The Skill and its Event assignments save together under the same revision and cross-reference validation as Events, preserving assignments belonging to other Skills. A referenced Skill cannot be deleted until its bindings are removed. Editing instructions never rewrites completed invocation or Round evidence.

## Future Direction

Improve reuse, discovery, and context selection while keeping authored instructions readable. Additional result contracts should remain Refine-owned so richer execution does not turn Skill editing into schema programming.
