// ---- Reporter onboarding ---------------------------------------------------
//
// Reporter identity remains browser-local (`refine_last_reporter`) and all
// durable Reporter changes still go through /api/reporters. This controller
// owns only the first-load orientation dialog and its modal lifecycle.

const reporterOnboardingScopes = new Map();
let activeReporterOnboarding = null;
let reporterOnboardingObserver = null;

function reporterOnboardingScopeKey() {
  if (!hasAttachedProject()) return "";
  const project = state.project || {};
  const projectKey = project.target_root || project.path || "attached-project";
  const activeNode = project.active_node;
  const nodeKey = project.active_node_id
    || (typeof activeNode === "string" ? activeNode : activeNode?.id)
    || "default";
  return `${projectKey}\u0000${nodeKey}`;
}

function reporterOnboardingSession(scopeKey) {
  let session = reporterOnboardingScopes.get(scopeKey);
  if (!session) {
    session = {
      scopeKey,
      outcome: "idle",
      hydrated: false,
      root: null,
      priorFocus: null,
      onKey: null,
      draft: "",
      error: "",
    };
    reporterOnboardingScopes.set(scopeKey, session);
  }
  return session;
}

function modalBackdrops() {
  return $$(".modal-backdrop").filter((backdrop) =>
    backdrop.querySelector('[aria-modal="true"]'));
}

function blockingReporterOnboardingModal(session) {
  return modalBackdrops().find((backdrop) => backdrop !== session?.root) || null;
}

function reporterOnboardingIsTopmost(session) {
  const backdrops = modalBackdrops();
  return !!session?.root && backdrops[backdrops.length - 1] === session.root;
}

function focusableReporterOnboardingControls(root) {
  return $$('button, input, select, textarea, [href], [tabindex]', root)
    .filter((control) => !control.disabled
      && !control.hidden
      && control.getAttribute("tabindex") !== "-1");
}

function stopReporterOnboardingKey(event) {
  event.preventDefault();
  if (typeof event.stopImmediatePropagation === "function") {
    event.stopImmediatePropagation();
  } else {
    event.stopPropagation();
  }
}

function removeReporterOnboarding(session, { restoreFocus = false } = {}) {
  const root = session?.root;
  if (!root) return;
  const priorFocus = session.priorFocus;
  const draft = root.querySelector("#reporter-onboarding-name")?.value;
  if (draft !== undefined) session.draft = draft;
  if (session.onKey) document.removeEventListener("keydown", session.onKey, true);
  session.onKey = null;
  session.root = null;
  root.remove();
  if (restoreFocus && priorFocus?.isConnected) priorFocus.focus();
}

function yieldReporterOnboarding(session) {
  if (!session?.root) return;
  if (session.outcome !== "submitting") session.outcome = "deferred";
  removeReporterOnboarding(session);
}

function completeReporterOnboarding(session) {
  session.outcome = "completed";
  session.error = "";
  removeReporterOnboarding(session);
}

function dismissReporterOnboarding(session) {
  if (!session?.root || !reporterOnboardingIsTopmost(session)) return;
  session.outcome = "dismissed";
  session.error = "";
  removeReporterOnboarding(session, { restoreFocus: true });
}

async function selectCreatedReporter(session, reporter) {
  completeReporterOnboarding(session);
  mergeReporterIntoProjection(reporter);
  populateAllReporterDropdowns();
  setLastReporter(reporter.name);
  try {
    await refreshReporters();
  } catch (error) {
    toast(
      `Reporter created and selected, but the Reporter list could not be refreshed: ${error.message}`,
      "warn",
      { source: "reporter-onboarding" },
    );
  }
}

function selectExistingReporter(session, name) {
  if (session.outcome === "submitting") return;
  const reporter = state.reporters.find((candidate) => candidate.name === name);
  if (!reporter) {
    session.outcome = "failed";
    session.error = "That Reporter is no longer available. Choose another Reporter.";
    reevaluateReporterOnboarding();
    return;
  }
  completeReporterOnboarding(session);
  setLastReporter(reporter.name);
}

function setReporterOnboardingBusy(session, busy) {
  if (!session.root) return;
  $$('button, input, select, textarea', session.root).forEach((control) => {
    control.disabled = busy;
  });
  const submit = session.root.querySelector('[data-testid="reporter-onboarding-create"]');
  if (submit) submit.textContent = busy ? "Creating…" : "Create Reporter";
}

function showReporterOnboardingError(session, message) {
  session.outcome = "failed";
  session.error = message;
  if (!session.root) {
    reevaluateReporterOnboarding();
    return;
  }
  setReporterOnboardingBusy(session, false);
  const error = session.root.querySelector("#reporter-onboarding-error");
  if (error) {
    error.textContent = message;
    error.hidden = false;
  }
  session.root.querySelector("#reporter-onboarding-name")?.focus();
}

async function submitReporterOnboarding(session, form) {
  if (session.outcome === "submitting") return;
  const input = form.elements.name;
  const name = (input?.value || "").trim();
  session.draft = input?.value || "";
  if (!name) {
    showReporterOnboardingError(session, "Enter your name to create a Reporter.");
    return;
  }

  session.outcome = "submitting";
  session.error = "";
  const error = session.root?.querySelector("#reporter-onboarding-error");
  if (error) error.hidden = true;
  setReporterOnboardingBusy(session, true);
  try {
    const result = await api("POST", "/api/reporters", { name });
    const reporter = result?.reporter;
    if (!reporter?.name) throw new Error("The Reporter service returned no Reporter.");
    await selectCreatedReporter(session, reporter);
  } catch (requestError) {
    showReporterOnboardingError(
      session,
      `Could not create Reporter: ${requestError.message || "Request failed"}`,
    );
  }
}

function openReporterOnboarding(session) {
  if (session.root || blockingReporterOnboardingModal(session)) return;
  session.outcome = session.outcome === "failed" ? "failed" : "open";
  session.priorFocus = document.activeElement;

  const root = document.createElement("div");
  root.className = "modal-backdrop reporter-onboarding-backdrop";
  root.innerHTML = `
    <div class="modal reporter-onboarding-modal" role="dialog" aria-modal="true"
         aria-labelledby="reporter-onboarding-title"
         aria-describedby="reporter-onboarding-description reporter-onboarding-guidance"
         data-testid="reporter-onboarding-dialog">
      <div class="modal-title" id="reporter-onboarding-title">Who are you?</div>
      <div class="modal-body">
        <p id="reporter-onboarding-description">
          Choose your Reporter so Refine can show the Goals and reviews that belong to you.
        </p>
        ${state.reporters.length ? `
          <div class="reporter-onboarding-choices" aria-label="Existing Reporters">
            ${state.reporters.map((reporter) => `
              <button type="button" class="secondary" data-reporter-onboarding-choice="${htmlEscape(reporter.name)}">
                ${htmlEscape(reporter.name)}
              </button>`).join("")}
          </div>
          <div class="reporter-onboarding-divider" aria-hidden="true">or create a Reporter</div>
        ` : `<p class="muted small">Create the first Reporter for this app.</p>`}
        <form id="reporter-onboarding-form">
          <label for="reporter-onboarding-name">Your name</label>
          <div class="reporter-onboarding-create-row">
            <input id="reporter-onboarding-name" name="name" type="text" class="modal-input"
                   autocomplete="name" value="${htmlEscape(session.draft)}">
            <button type="submit" data-testid="reporter-onboarding-create">Create Reporter</button>
          </div>
          <div class="form-error" id="reporter-onboarding-error" role="alert" aria-live="assertive"
               ${session.error ? "" : "hidden"}>${htmlEscape(session.error)}</div>
        </form>
        <p class="muted small reporter-onboarding-guidance" id="reporter-onboarding-guidance">
          You can change this anytime under <strong>Controls &gt; Reporter</strong>.
        </p>
      </div>
      <div class="modal-actions">
        <button type="button" class="secondary" data-testid="reporter-onboarding-dismiss">Not now</button>
      </div>
    </div>`;

  session.root = root;
  activeReporterOnboarding = session;
  document.body.appendChild(root);

  session.onKey = (event) => {
    if (!reporterOnboardingIsTopmost(session)) return;
    if (event.key === "Escape") {
      stopReporterOnboardingKey(event);
      dismissReporterOnboarding(session);
      return;
    }
    if (event.key !== "Tab") return;
    const controls = focusableReporterOnboardingControls(root);
    if (!controls.length) return;
    const current = document.activeElement;
    const currentIndex = controls.indexOf(current);
    if (currentIndex < 0) {
      stopReporterOnboardingKey(event);
      controls[event.shiftKey ? controls.length - 1 : 0].focus();
    } else if (!event.shiftKey && currentIndex === controls.length - 1) {
      stopReporterOnboardingKey(event);
      controls[0].focus();
    } else if (event.shiftKey && currentIndex === 0) {
      stopReporterOnboardingKey(event);
      controls[controls.length - 1].focus();
    }
  };
  document.addEventListener("keydown", session.onKey, true);

  root.addEventListener("click", (event) => {
    if (event.target === root) dismissReporterOnboarding(session);
  });
  root.querySelector('[data-testid="reporter-onboarding-dismiss"]')
    .addEventListener("click", () => dismissReporterOnboarding(session));
  $$('[data-reporter-onboarding-choice]', root).forEach((button) => {
    button.addEventListener("click", () =>
      selectExistingReporter(session, button.dataset.reporterOnboardingChoice));
  });
  root.querySelector("#reporter-onboarding-form").addEventListener("submit", (event) => {
    event.preventDefault();
    submitReporterOnboarding(session, event.currentTarget);
  });

  const initialFocus = root.querySelector('[data-reporter-onboarding-choice]')
    || root.querySelector("#reporter-onboarding-name");
  initialFocus?.focus();
}

function reevaluateReporterOnboarding() {
  const scopeKey = reporterOnboardingScopeKey();
  if (!scopeKey) {
    if (activeReporterOnboarding?.root) yieldReporterOnboarding(activeReporterOnboarding);
    activeReporterOnboarding = null;
    return;
  }

  const session = reporterOnboardingSession(scopeKey);
  if (activeReporterOnboarding && activeReporterOnboarding !== session) {
    yieldReporterOnboarding(activeReporterOnboarding);
  }
  activeReporterOnboarding = session;

  const hasValidSelection = !!state.lastReporter
    && state.reporters.some((reporter) => reporter.name === state.lastReporter);
  if (hasValidSelection) {
    if (!["dismissed", "completed"].includes(session.outcome)) {
      completeReporterOnboarding(session);
    } else if (session.root) {
      removeReporterOnboarding(session);
    }
    return;
  }
  if (!session.hydrated || ["dismissed", "completed", "submitting"].includes(session.outcome)) {
    return;
  }

  const blocker = blockingReporterOnboardingModal(session);
  if (blocker) {
    yieldReporterOnboarding(session);
    if (session.outcome !== "failed") session.outcome = "deferred";
    return;
  }
  openReporterOnboarding(session);
}

function observeReporterOnboardingModals() {
  if (reporterOnboardingObserver) return;
  reporterOnboardingObserver = new MutationObserver(() => {
    const session = activeReporterOnboarding;
    if (session?.root && blockingReporterOnboardingModal(session)) {
      yieldReporterOnboarding(session);
    }
    reevaluateReporterOnboarding();
  });
  reporterOnboardingObserver.observe(document.body, { childList: true, subtree: true });
}

function notifyReporterOnboardingHydrated() {
  const scopeKey = reporterOnboardingScopeKey();
  if (!scopeKey) return;
  observeReporterOnboardingModals();
  reporterOnboardingSession(scopeKey).hydrated = true;
  reevaluateReporterOnboarding();
}
