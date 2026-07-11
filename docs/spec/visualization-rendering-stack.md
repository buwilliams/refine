# Interactive Generative Simulation Gallery Rendering Stack

## Decision

The visualization gallery will use p5.js in instance mode as its primary
rendering stack.

p5.js is a fit for this app because the product is centered on live
generative-physics and emergent-systems sketches. It gives future sketches a
compact authoring model for animation loops, canvas input, drawing primitives,
and rapid parameter exploration without hiding the underlying canvas model.

## Stack Shape

The app should treat each sketch as a small module with a stable lifecycle:

- `setup(context)` creates sketch-local state and registers controls.
- `draw(state)` advances the simulation and renders one frame.
- `reset()` restores deterministic defaults.
- `capture()` returns a canvas image for sharing or thumbnails.
- `resize(width, height)` adapts to its container without recreating app state.

The gallery shell owns navigation, chapter grouping, parameter panels, reset
actions, capture/share actions, and responsive layout. Sketch modules own only
their simulation state, p5 drawing logic, and sketch-specific controls.

## Runtime Contract

Sketches run as p5 instance-mode sketches mounted into a gallery-managed
container. The gallery passes each sketch a bounded context with:

- Container dimensions.
- Seed and reset hooks.
- Pointer and keyboard input helpers.
- Parameter registration for sliders, toggles, selects, and numeric inputs.
- Capture helpers that read from the active canvas.

The app should avoid global p5 mode so multiple live or preview sketches can be
mounted independently later. Instance mode also keeps cleanup predictable when
users switch chapters or open a sketch detail view.

## Controls And Input

Parameter controls should be ordinary browser controls outside the canvas. The
canvas remains focused on drawing and direct manipulation, while the shell keeps
controls accessible, keyboard reachable, and serializable for sharing.

Pointer input should be normalized before it reaches a sketch. Mouse, stylus,
and touch events should share a single pointer model so later mobile work can
add touch parity without rewriting sketch internals.

## Preview Strategy Boundary

This decision does not require all gallery tiles to run live p5 instances.
Preview behavior is a separate Goal. The rendering stack should support both
static captured thumbnails and live low-fidelity previews by keeping sketch
lifecycle methods deterministic and cheap to mount.

## Performance Boundary

p5.js will be used for the first implementation path. High-entity sketches
should keep simulation logic separate from drawing so later optimizations can
add spatial partitioning, typed arrays, workers, or raw Canvas/WebGL renderers
inside individual sketches when needed. Those optimizations should remain
behind the same sketch lifecycle contract.

## Acceptance

- The app has a named rendering technology: p5.js in instance mode.
- Future sketches have a documented lifecycle and mounting contract.
- Parameter controls, canvas input, resizing, reset, capture, and future sketch
  authoring are covered by the stack decision.
- Later preview, touch, and performance Goals can build on this contract without
  changing the selected stack.
