# Mission

## Key Ideas

- **A Mission Is A Larger Goal**: the same shape of work at a larger scope. Mission and Goal differ in purpose and in what they produce, not in kind.
- **Different Outputs**: a Goal produces repository change. A Mission produces high-level plans, gathered data and insight, and the Features and Goals that carry the work.
- **Only Through Goals**: a Mission never changes the target application and never gathers on its own. Everything it learns and everything it changes happens through Goals.
- **Container, Not Supervisor**: a Mission holds a set of Features and Goals and is not complete until they are. Child work is what a Mission is made of, not a signal it watches from outside.
- **Planning Differs Only In Scope**: Mission planning proposes, criticizes, and revises exactly as Goal planning does, carrying only the detail that matters at Mission scope. Concrete implementation detail belongs to each Goal's own planning.
- **Neighbours Are Visible**: a Goal in a Mission knows the stated outcome, the Mission's plan and gathered insight, and the other Goals. Work that can see its neighbours does not rediscover what they established.
- **Rounds Reach The Outcome**: a Mission converges the way a Goal does — quality and governance findings draft another Round, and a person accepts the result at review.

## Purpose

Large outcomes do not fail inside Goals. They fail between them, in two ways.

Understanding evaporates. An agent works out how a system fits together, changes it, and ends its conversation. The next agent starts over. Across many Goals the same system is rediscovered many times, badly, and no two agents hold the same picture of it.

Nothing re-plans. Work drafted before anything ran does not know what the work found. The Goals complete, the drafted work is exhausted, and the outcome the person wanted is still not reached — with nothing in the system whose job it was to notice.

Mission closes both gaps and does nothing else. Refine already executes a Goal well: it plans, implements, checks quality, applies governance, reviews, and merges. Mission is that same work one scope up. It plans what should exist, produces the Goals that do it, keeps what they learn, and is not finished until the outcome is.

Feature does not fill this role and should not be asked to. Feature groups Goals and preserves their order. It has no planning, no gathered knowledge, and no completion of its own. Mission is the concept with a lifecycle above the Goal.

## Expected Role

Goal executes. Feature groups and orders. **Mission plans and contains.**

### What a Mission produces

A Mission's outputs are high-level plans, the data and insight its work gathered, and the Features and Goals that carry the work forward. A Goal's output is repository change. That difference in product, not a difference in kind, is what separates the two.

Gathered insight is an output, not a store. What the work learned about the product is a product artifact: it is written into the repository by the Goal that gathered it, and it passes through the same quality, governance, review, and integration as any other change. It is then readable by anyone who opens the repository, including agents that never knew a Mission existed.

Refine keeps the reference, not the content. The Mission's own record holds its statement, its plans, its Round evidence, and pointers to what its Goals produced — path, commit, and originating Goal — exactly as Goal records already carry their target branch, base commit, and candidate commit. Refine should hold no private model of what a Mission knows; giving it custody of the content is what invites an artifact registry, promotion rules, and a ledger to keep correct.

### Working only through Goals

A Mission does not open the target application, gather from it directly, hold a worktree, or produce a candidate branch. Understanding the system is Goal work; changing it is Goal work. The Mission plans that work and receives what it produced.

This keeps every application change under the existing quality, governance, and review authority, with no second path into the repository.

### Waiting for its Goals

A Mission is not complete when its plan is made or its Goals are created. It waits for them. Anything else would present a Mission as finished while the work it exists to do is still running.

### Mission context for Goals

A Goal that belongs to a Mission receives the stated outcome, the Mission's plan, what earlier work gathered, and knowledge of its sibling Goals. This context is given deliberately and legibly, as its own part of what the Goal agent is told — never smuggled in as unlabelled additional context.

A Goal outside a Mission is unaffected and behaves exactly as it does today.

### Planning at Mission scope

Mission planning is the same design as Goal planning — proposal, independent criticism, revision — differing only in what it is trying to achieve. It carries the detail that matters at Mission scope and leaves concrete implementation detail to each Goal's own planning, where it belongs and where the context to decide it exists.

Its result is reviewable before it becomes durable work, and it is stated in the language the person used, not as a digest to authorize.

### Reaching the outcome

Quality and governance judge the combined result of the Mission's work. A finding drafts another Round, which plans further Goals against what has been learned. Review is where a person accepts the result. A Mission is Done because the outcome is reached and a person agreed, never because a count of children reached zero.

Nothing separate watches for convergence. The Round is the mechanism, exactly as it is for a Goal.

### Two entities, similar workflows

Mission and Goal are separate records with separate purposes. Their workflows are similar enough that one shared implementation is worth attempting, and worth attempting only if it comes out clean; a forced generalization would cost more than it saves and would be a worse answer than two honest implementations.

One phase genuinely differs in kind. A Goal implements through one bounded agent working in an isolated worktree. A Mission's execution is an open-ended wait on the work it created. Planning, quality, governance, and review have the same shape at both scopes. Whether that single difference can be expressed cleanly inside one workflow is an open engineering question, not a settled part of this intent.

### What Mission must not become

Learned from a first implementation that was withdrawn, which began by denying that a Mission is a larger Goal and then had to rebuild everything it had refused to share:

- **A second workflow vocabulary.** A parallel status enum, quality, governance, and review are the cost of that denial, not a design.
- **A knowledge store.** What a Mission learned belongs in reviewed, version-controlled work products, not in a private graph of claims that Refine has to keep correct.
- **A context compiler.** Deciding what a worker needs to know is among the strongest things a capable model does. It must not be reduced to matching identifiers.
- **A ceremony of digests.** Approval is a person agreeing with a direction stated in their own words.
- **A queue of adjudications.** If the work routinely asks a person to settle individual findings, the planning was wrong, not the world.

Mission is currently a design without an implementation; an earlier one was removed rather than completed. This document states what a new implementation must preserve, not what exists.

## Future Direction

As agents improve, Mission should get thinner rather than thicker. More of the planning, reduction, and judgment should collapse into fewer and more capable agent passes, while Refine's part shrinks toward what only it can provide: durable identity, membership, ordering, evidence, the human gates, and recovery.

The durable value a Mission leaves behind is what its Goals gathered and wrote down. Those work products should outlive the Mission that produced them, the fleet that ran it, and Refine's own mechanisms — readable by a person, by the next agent, and by a later Mission that simply reads them the way any work reads its context. Reuse across Missions needs no new concept.

The direction is many Missions running in parallel across a fleet, each holding one stated outcome, each producing the work that reaches it.
