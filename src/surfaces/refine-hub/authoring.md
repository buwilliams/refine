# Update Refine Hub

Run **Update Refine Hub** from the Settings → Workspace controls & support, or choose **Run Skill** next to Refine Hub in Settings → Hubs. Describe what readers should understand after the update. For release notes, include the version and coverage dates.

The Skill contains the instructions for maintaining product documentation and release notes. You can edit those instructions in Settings → Prompts. This built-in Skill cannot be removed.

Refine Hub ships with Refine. Its Skill updates the source documentation; users receive changes when they update Refine.

## Maintain your own Hub

When creating a Hub, choose a project Skill that knows how to build and maintain it, or create one. Creating the Hub saves the association without starting an agent.

Use **Run Skill** in the Hub to build it or refresh its content. For automatic updates, assign the same Skill to events in Settings → Prompts. Add a Custom action assignment to make it manually runnable.

The `{{hubs}}` prompt variable contains the associated Hub details. A manual run from a Hub includes only that Hub; event runs include every Hub maintained by that Skill. These details are saved with the invocation and included through editable prompt templates.

Write the Skill’s instructions to describe the desired pages or data, where to obtain it, and when to publish. User Hub content and its Skill association synchronize through the target app’s refine-state repository. Published content becomes available to others connected to that state; this does not publish it to the public internet.
