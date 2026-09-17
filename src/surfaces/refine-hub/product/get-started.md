# Get started

Refine coordinates your agents around Goals: descriptions of the outcomes you want for your target application.

## Install Refine

Follow the [installation runbook](../docs/runbooks/install.md), then open the address printed by Refine.

## Connect your target application

Open **Settings → Nodes** and attach the repository you want agents to work on. Configure the node for this machine and select an available agent provider. The [product reference](reference.md) explains the available settings.

## Create a Goal

Choose your Node and Reporter in the main navigation. Open **New Goal**, describe the current behavior and the outcome you want, and save it. Add enough detail for someone else to understand what success looks like.

## Follow the work

The agent workflow is **Plan → Implement → Quality → Governance**. Planning clarifies the work, Implement changes the code, Quality tests the outcome, and Governance verifies alignment with the Goal and project.

Open the Goal to follow its rounds and activity. Its **Prompts** tab shows retained launch prompts, making it easier to understand the context each agent received. Older runs may not have a retained prompt.

## Make Refine fit your project

Use **Settings → Prompts** to manage Skills and Templates. Start with the defaults; change the parts that help agents understand your project's purpose and architecture. See [Customize agent prompts](prompts.md).

Open **Control** to inspect processes and use the target app’s Build, Start/Stop, and status-check actions. Worker rows expose their supported actions. Use **Tools** for Files, System, terminals, and agents.

Expand **Skills** or **Hubs** in the rail for their launch, Add, and Manage actions. See [Find your way around Refine](navigation.md) for a guide to the rail, icon actions, and Search.
