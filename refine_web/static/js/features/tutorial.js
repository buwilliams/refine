// ---- Tutorial ---------------------------------------------------------------
//
// A modal walkthrough of the 5 main surfaces. The `#/tutorial` route is
// modal-only — `#main` is left blank because the modal IS the page. Closing
// the modal (X, Escape, "Done", backdrop, or any other nav click) drops the
// user back at the dashboard so the URL no longer points at a torn-down page.

const TUTORIAL_STEPS = [
  {
    image: "/static/ss-1-dashboard.png",
    title: "Verified software, by ordinary people enhanced by agents",
    body: "Refine turns software gaps — features and bugs — into verified software. QA, Product, support, customers: anyone who can articulate what the app does today vs what it should do instead submits a Gap.",
  },
  {
    image: "/static/ss-2-gaps.png",
    title: "Describe the gap, not the fix",
    body: "The unit of work is a Gap: a name, what the app does today, and what it should do instead. You don't need to know how to fix it — you need to describe the gap between the two. The agent handles the “how.”",
  },
  {
    image: "/static/ss-3-logs.png",
    title: "Stay in the loop without becoming an engineer",
    body: "Every action — agent runs, git operations, merges, status transitions — streams to a filterable Logs view in real time. Watch the agent work, or read the transcript after.",
  },
  {
    image: "/static/ss-4-changes.png",
    title: "The human gate",
    body: "Each Gap runs in its own git worktree and produces a diff. The Changes view is where you look at what the agent did. A Gap doesn't close until someone verifies it — the agent never marks its own work done.",
  },
  {
    image: "/static/ss-5-chat.png",
    title: "Talk to the work",
    body: "Open a Gap and the chat dock primes an agent session with that Gap's full context. Ask “why did you change this?” or “what about edge case X?” without re-explaining anything. The conversation lives next to the work.",
  },
];

let _tutorialModalRoot = null;

async function renderTutorial() {
  // Modal-only route — leave the page body blank; the modal is the surface.
  $("#main").innerHTML = "";
  openTutorialModal();
}

function closeTutorialModal() {
  if (!_tutorialModalRoot) return;
  const root = _tutorialModalRoot;
  _tutorialModalRoot = null;
  root.remove();
}

function openTutorialModal() {
  if (_tutorialModalRoot) return;
  let step = 0;

  const root = document.createElement("div");
  root.className = "modal-backdrop tutorial-backdrop";
  root.innerHTML = `
    <div class="modal tutorial-modal" role="dialog" aria-modal="true" aria-labelledby="tutorial-title">
      <button class="tutorial-close" type="button" aria-label="Close tutorial">×</button>
      <div class="tutorial-image-wrap">
        <img class="tutorial-image" alt="">
      </div>
      <div class="tutorial-copy">
        <div class="tutorial-step-label"></div>
        <div class="modal-title tutorial-title" id="tutorial-title"></div>
        <div class="tutorial-body"></div>
      </div>
      <div class="tutorial-footer">
        <div class="tutorial-dots" role="tablist"></div>
        <div class="tutorial-nav">
          <button type="button" class="secondary tutorial-prev">Back</button>
          <button type="button" class="tutorial-next">Next</button>
        </div>
      </div>
    </div>
  `;
  document.body.appendChild(root);
  _tutorialModalRoot = root;

  const img = root.querySelector(".tutorial-image");
  const titleEl = root.querySelector(".tutorial-title");
  const stepLabel = root.querySelector(".tutorial-step-label");
  const bodyEl = root.querySelector(".tutorial-body");
  const dotsEl = root.querySelector(".tutorial-dots");
  const prevBtn = root.querySelector(".tutorial-prev");
  const nextBtn = root.querySelector(".tutorial-next");
  const closeBtn = root.querySelector(".tutorial-close");

  for (let i = 0; i < TUTORIAL_STEPS.length; i++) {
    const d = document.createElement("button");
    d.type = "button";
    d.className = "tutorial-dot";
    d.dataset.idx = String(i);
    d.setAttribute("role", "tab");
    d.setAttribute("aria-label", `Step ${i + 1} of ${TUTORIAL_STEPS.length}`);
    dotsEl.appendChild(d);
  }

  function render() {
    const s = TUTORIAL_STEPS[step];
    img.src = s.image;
    img.alt = s.title;
    titleEl.textContent = s.title;
    bodyEl.textContent = s.body;
    stepLabel.textContent = `Step ${step + 1} of ${TUTORIAL_STEPS.length}`;
    prevBtn.disabled = step === 0;
    const last = step === TUTORIAL_STEPS.length - 1;
    nextBtn.textContent = last ? "Done" : "Next";
    for (const dot of root.querySelectorAll(".tutorial-dot")) {
      const isActive = Number(dot.dataset.idx) === step;
      dot.classList.toggle("active", isActive);
      dot.setAttribute("aria-selected", isActive ? "true" : "false");
    }
  }

  function dismiss() {
    document.removeEventListener("keydown", onKey, true);
    closeTutorialModal();
    // Drop the URL back to the dashboard so the user isn't stranded on a
    // route whose only surface (the modal) is no longer open.
    if (location.hash === "#/tutorial") location.hash = "#/";
  }

  function onKey(e) {
    if (e.key === "Escape") { e.preventDefault(); dismiss(); return; }
    if (e.key === "ArrowRight") {
      e.preventDefault();
      if (step < TUTORIAL_STEPS.length - 1) { step++; render(); }
      else dismiss();
      return;
    }
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (step > 0) { step--; render(); }
      return;
    }
  }
  document.addEventListener("keydown", onKey, true);

  root.addEventListener("click", (e) => {
    if (e.target === root) dismiss();
  });
  closeBtn.addEventListener("click", dismiss);
  prevBtn.addEventListener("click", () => {
    if (step > 0) { step--; render(); }
  });
  nextBtn.addEventListener("click", () => {
    if (step < TUTORIAL_STEPS.length - 1) { step++; render(); }
    else dismiss();
  });
  dotsEl.addEventListener("click", (e) => {
    const dot = e.target.closest(".tutorial-dot");
    if (!dot) return;
    step = Number(dot.dataset.idx);
    render();
  });

  render();
  nextBtn.focus();
}
