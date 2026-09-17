// Shared project boards. Goal lifecycle remains authoritative; lanes organize intent.
let planningSnapshot = null;
let planningRefresh = 0;
let planningBoardId = null;
let planningShowArchived = false;

function planningRequestId() {
  return typeof hubId === "function"
    ? hubId()
    : `planning-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}
function planningCards(boardId, laneId) {
  return (planningSnapshot?.cards || [])
    .filter(
      (c) =>
        c.placement.board_id === boardId &&
        c.placement.lane_id === laneId &&
        (planningShowArchived || !c.placement.archived),
    )
    .sort(
      (a, b) =>
        a.placement.position - b.placement.position ||
        a.placement.goal_id.localeCompare(b.placement.goal_id),
    );
}
async function planningCommand(
  operation,
  fields = {},
  data = {},
  requestId = planningRequestId(),
) {
  const action = await api("POST", "/api/planning/commands", {
    operation,
    request_id: requestId,
    actor: state.lastReporter || "operator",
    ...fields,
    data,
  });
  if (action.state === "failed")
    throw new Error(action.message || "Planning action failed");
  return action;
}
function planningSubmitter(operation) {
  let previous = null,
    requestId = null;
  return (fields, data = {}) => {
    const encoded = JSON.stringify({ fields, data });
    if (encoded !== previous) {
      previous = encoded;
      requestId = planningRequestId();
    }
    return planningCommand(operation, fields, data, requestId);
  };
}
async function renderPlanning() {
  if (
    typeof renderNoProjectIfDetached === "function" &&
    renderNoProjectIfDetached("Project Planning")
  )
    return;
  const main = document.getElementById("main");
  main.innerHTML =
    '<section class="planning-page"><h1>Project Planning</h1><p role="status">Loading shared boards…</p></section>';
  await refreshPlanning();
}
async function refreshPlanning() {
  const generation = ++planningRefresh;
  const nodeGeneration = captureNodeContextGeneration();
  try {
    const snapshot = await api("GET", "/api/planning", undefined, {
      cache: false,
    });
    if (
      generation !== planningRefresh ||
      state.currentRoute !== "planning" ||
      !isNodeContextGenerationCurrent(nodeGeneration)
    )
      return;
    if (document.querySelector(".planning-composer, .planning-dragging"))
      return;
    planningSnapshot = snapshot;
    const linkedCard = snapshot.cards.find(
      (c) =>
        c.placement.goal_id ===
        new URLSearchParams(location.hash.split("?")[1] || "").get("card"),
    );
    if (
      linkedCard?.placement.archived ||
      snapshot.boards.find((b) => b.id === linkedCard?.placement.board_id)
        ?.archived
    )
      planningShowArchived = true;
    const boards = snapshot.boards.filter(
      (b) => planningShowArchived || !b.archived,
    );
    const requested = new URLSearchParams(
      location.hash.split("?")[1] || "",
    ).get("board");
    let board =
      boards.find(
        (b) =>
          b.id ===
          (requested || linkedCard?.placement.board_id || planningBoardId),
      ) || boards[0];
    planningBoardId = board?.id || null;
    renderPlanningNavigation(snapshot);
    const esc = htmlEscape;
    const actions = snapshot.actions.filter(
      (a) =>
        a.state !== "complete" &&
        (!board ||
          a.command.board_id === board.id ||
          snapshot.cards.some(
            (c) =>
              c.placement.board_id === board.id &&
              c.placement.goal_id === a.command.goal_id,
          )),
    );
    const previousPage = document.querySelector(".planning-page");
    const previousScroll =
      previousPage?.dataset.boardId === board?.id
        ? previousPage.querySelector(".planning-lanes")?.scrollLeft || 0
        : 0;
    const focused = document.activeElement;
    const focusAttribute = focused?.closest(".planning-page")
      ? [...focused.attributes].find((attribute) =>
          attribute.name.startsWith("data-"),
        )
      : null;
    document.getElementById("main").innerHTML =
      `<section class="planning-page" data-testid="planning-page" data-board-id="${esc(board?.id || "")}">
      <div class="planning-heading"><div><h1>Project Planning</h1><p class="muted">A shared space for ideas, personal tasks, and work ready for your nodes.</p></div><div class="actions"><button data-planning-new-board>New board</button><button class="secondary" data-planning-refresh>Refresh</button></div></div>
      <div class="planning-toolbar"><div class="planning-board-heading"><h2>${esc(board?.name || "Your boards")}</h2><span class="muted">${board ? `${board.lanes.length} lanes · ${snapshot.cards.filter((c) => c.placement.board_id === board.id && !c.placement.archived).length} cards` : "Create your first board to get started"}</span></div>
      <button type="button" class="planning-archive-toggle" role="switch" aria-checked="${planningShowArchived}" data-planning-archived><span class="planning-toggle-track" aria-hidden="true"></span>Show archived</button>
      ${board ? '<button class="secondary" data-planning-board-settings>Board settings</button><button data-planning-new-lane>Add lane</button>' : ""}
      ${!snapshot.migration ? '<button class="secondary" data-planning-migrate>Import Todo Lists</button>' : ""}</div>
      ${board?.archived ? '<p class="muted">This board is archived. Restore it in Board settings to add new cards.</p>' : ""}
      ${(snapshot.errors || []).map((error) => `<p role="alert">${esc(error)}</p>`).join("")}
      ${actions.length ? `<section class="planning-actions" aria-label="Planning actions">${actions.map((a) => `<div role="status"><strong>${esc({ queued: "Queued", waiting: "Waiting", failed: "Needs attention", cancelled: "Cancelled" }[a.state] || a.state)}</strong> · ${esc({ "card.create": "Create card", "card.move": "Move card", "card.attach": "Add existing Goal", "card.apply": "Release card", "card.update": "Update card" }[a.command.operation] || "Board update")} · ${esc(a.message || `Processing on ${a.owner}`)} <button class="secondary" data-planning-action="${esc(a.id)}">Details</button>${!["failed", "cancelled"].includes(a.state) ? `<button class="secondary" data-planning-cancel="${esc(a.id)}">Cancel</button>` : ""}</div>`).join("")}</section>` : ""}
      ${
        board
          ? `<div class="planning-lanes" aria-label="${esc(board.name)}">${board.lanes
              .map(
                (
                  lane,
                ) => `<section class="planning-lane" data-lane="${esc(lane.id)}" data-lane-action="${esc(lane.action)}" aria-label="${esc(lane.name)}">
        <header><h2>${esc(lane.name)} <span class="planning-lane-count">${planningCards(board.id, lane.id).length}</span></h2><button class="secondary" data-lane-settings="${esc(lane.id)}" aria-label="Settings for ${esc(lane.name)}">⋯</button></header>
        <p class="planning-lane-action">${lane.action === "release" ? "Releases work" : lane.action === "accept_into_backlog" ? "Accepts into Backlog" : "Organize ideas and tasks"}</p>
        <div class="planning-card-list">${planningCards(board.id, lane.id)
          .map((card) => planningCardHtml(card, board))
          .join("")}</div>
        <div class="planning-lane-footer"><button class="secondary" data-planning-add="${esc(lane.id)}" ${board.archived ? "disabled" : ""}><span aria-hidden="true">＋</span> Add card</button></div>
      </section>`,
              )
              .join("")}</div>`
          : '<div class="planning-empty"><h2>Make room for your next idea</h2><p>Create a board, add lanes, and collect cards. Choose when your ideas enter the workflow.</p></div>'
      }
      </section>`;
    bindPlanning(board);
    const lanes = document.querySelector(".planning-lanes");
    if (lanes) lanes.scrollLeft = previousScroll;
    if (focusAttribute) {
      [...document.querySelectorAll(`.planning-page [${focusAttribute.name}]`)]
        .find(
          (element) =>
            element.getAttribute(focusAttribute.name) ===
              focusAttribute.value && element.className === focused.className,
        )
        ?.focus({ preventScroll: true });
    }
    const requestedCard = new URLSearchParams(
      location.hash.split("?")[1] || "",
    ).get("card");
    if (requestedCard) {
      const el = [...document.querySelectorAll("[data-card]")].find(
        (el) => el.dataset.card === requestedCard,
      );
      el?.focus();
      el?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  } catch (error) {
    if (state.currentRoute === "planning") showActionError(error);
  }
}
function planningCardHtml(card, board) {
  const p = card.placement,
    goal = card.goal || {},
    esc = htmlEscape;
  return `<article class="planning-card" draggable="${!board.archived}" data-card="${esc(p.goal_id)}" tabindex="0" aria-label="${esc(goal.name || p.goal_id)}">
    ${goal.status === "draft" ? `<button class="planning-card-title" data-card-edit="${esc(p.goal_id)}">${esc(goal.name || p.goal_id)}</button>` : `<a class="planning-card-title" href="#/goals/${encodeURIComponent(p.goal_id)}">${esc(goal.name || p.goal_id)}</a>`}
    <span class="planning-status status-${esc(goal.status || "draft")}">${esc(workflowStatusLabel(goal.status || "draft"))}</span>
    ${goal.description ? `<p>${esc(goal.description.slice(0, 180))}</p>` : ""}
    ${card.error ? `<p role="alert">${esc(card.error)}</p>` : ""}
    <p class="planning-card-meta">${esc(goal.reporter || "No Reporter")} · ${esc(goal.priority || "low")} priority${goal.status !== "draft" ? `<br>Node: ${esc(goal.node_display_name || goal.node_id || "unknown")}` : ""}</p>
    <div class="actions">${card.error ? `<button class="secondary" data-card-detach="${esc(p.goal_id)}">Remove from board</button>` : `<button class="secondary" data-card-edit="${esc(p.goal_id)}">Edit</button>`}<button class="secondary" data-card-move="${esc(p.goal_id)}">Move</button><button class="secondary" data-card-archive="${esc(p.goal_id)}">${p.archived ? "Restore" : "Archive"}</button></div>
    ${board.lanes.find((l) => l.id === p.lane_id)?.action !== "none" ? `<button class="secondary" data-card-apply="${esc(p.goal_id)}">${board.lanes.find((l) => l.id === p.lane_id)?.action === "release" ? "Release to execution" : "Send to Backlog"}</button>` : ""}</article>`;
}
function bindPlanning(board) {
  const root = document.querySelector(".planning-page");
  const run = (fn) => async (event) => {
    try {
      await fn(event);
      await refreshPlanning();
    } catch (e) {
      showActionError(e);
    }
  };
  root.querySelector("[data-planning-new-board]").onclick = () =>
    planningEdit("New board", "board.create", {}, { name: "", routing: "" });
  root.querySelector("[data-planning-refresh]").onclick = refreshPlanning;
  root.querySelector("[data-planning-archived]").onclick = async (event) => {
    planningShowArchived = !planningShowArchived;
    event.currentTarget.setAttribute(
      "aria-checked",
      String(planningShowArchived),
    );
    await refreshPlanning();
    document.querySelector("[data-planning-archived]")?.focus();
  };
  root.querySelector("[data-planning-migrate]")?.addEventListener(
    "click",
    run(() => planningCommand("migrate")),
  );
  root
    .querySelectorAll("[data-planning-cancel]")
    .forEach(
      (el) =>
        (el.onclick = run(() =>
          api(
            "POST",
            `/api/planning/actions/${encodeURIComponent(el.dataset.planningCancel)}/cancel`,
            {},
          ),
        )),
    );
  root.querySelectorAll("[data-planning-action]").forEach(
    (el) =>
      (el.onclick = () => {
        const a = planningSnapshot.actions.find(
          (a) => a.id === el.dataset.planningAction,
        );
        hubModal(
          "Planning action",
          `<p>${htmlEscape(a.message || a.state)}</p><p>Current processor: ${htmlEscape(a.owner)}<br>Execution target: ${htmlEscape(a.target_node || "Not selected")}</p><p>Step: ${htmlEscape(a.phase)}<br>Requested by: ${htmlEscape(a.command.actor)}<br>Created: ${htmlEscape(a.created)}</p>${a.result?.goal_id ? `<p><a href="#/goals/${encodeURIComponent(a.result.goal_id)}">Open Goal</a></p>` : ""}${
            Object.keys(a.invocations || {}).length
              ? `<p>Skill invocations</p><ul>${Object.entries(a.invocations)
                  .map(
                    ([edge, id]) =>
                      `<li>${htmlEscape(edge)}: ${htmlEscape(id)}</li>`,
                  )
                  .join("")}</ul>`
              : ""
          }`,
        );
      }),
  );
  if (!board) return;
  root.querySelector("[data-planning-board-settings]").onclick = () =>
    planningEdit(
      "Board settings",
      "board.update",
      { board_id: board.id, expected_revision: board.revision },
      { name: board.name, routing: board.routing || "" },
      board,
    );
  root.querySelector("[data-planning-new-lane]").onclick = () =>
    planningEdit(
      "New lane",
      "lane.create",
      { board_id: board.id, expected_revision: board.revision },
      { name: "", action: "none", routing: "" },
    );
  root.querySelectorAll("[data-lane-settings]").forEach(
    (el) =>
      (el.onclick = () => {
        const l = board.lanes.find((l) => l.id === el.dataset.laneSettings);
        planningEdit(
          "Lane settings",
          "lane.update",
          {
            board_id: board.id,
            lane_id: l.id,
            expected_revision: board.revision,
          },
          { name: l.name, action: l.action, routing: l.routing || "" },
          board,
        );
      }),
  );
  root.querySelectorAll("[data-planning-add]").forEach((button) => {
    button.onclick = () =>
      planningOpenComposer(board, button.dataset.planningAdd, button);
  });
  const card = (id) =>
    planningSnapshot.cards.find((c) => c.placement.goal_id === id);
  const fields = (c) => ({
    goal_id: c.placement.goal_id,
    expected_revision: c.placement.revision,
    board_id: board.id,
  });
  root
    .querySelectorAll("[data-card-detach]")
    .forEach(
      (el) =>
        (el.onclick = run(() =>
          planningCommand("card.detach", fields(card(el.dataset.cardDetach))),
        )),
    );
  root.querySelectorAll("[data-card-edit]").forEach(
    (el) =>
      (el.onclick = () => {
        const c = card(el.dataset.cardEdit);
        planningEdit("Edit card", "card.update", fields(c), {
          name: c.goal.name,
          ...(["draft", "backlog"].includes(c.goal.status)
            ? { description: c.goal.description || "" }
            : {}),
          reporter: c.goal.reporter || "",
          priority: c.goal.priority,
          expected_goal_revision: c.goal.workflow_revision || 0,
          routing: c.placement.routing || "",
        });
      }),
  );
  root
    .querySelectorAll("[data-card-move]")
    .forEach(
      (el) => (el.onclick = () => planningMove(card(el.dataset.cardMove))),
    );
  root.querySelectorAll("[data-card-archive]").forEach(
    (el) =>
      (el.onclick = run(() => {
        const c = card(el.dataset.cardArchive);
        return planningCommand("card.archive", fields(c), {
          archived: !c.placement.archived,
        });
      })),
  );
  root
    .querySelectorAll("[data-card-apply]")
    .forEach(
      (el) =>
        (el.onclick = run(() =>
          planningCommand("card.apply", fields(card(el.dataset.cardApply))),
        )),
    );
  let draggedId = null;
  const clearDrop = () => {
    root
      .querySelectorAll(
        ".planning-drop-before, .planning-drop-after, .planning-drop-lane",
      )
      .forEach((el) =>
        el.classList.remove(
          "planning-drop-before",
          "planning-drop-after",
          "planning-drop-lane",
        ),
      );
  };
  root.querySelectorAll("[data-card]").forEach((el) => {
    el.ondragstart = (e) => {
      if (board.archived || e.target.closest("input, textarea, select")) {
        e.preventDefault();
        return;
      }
      draggedId = el.dataset.card;
      e.dataTransfer.setData("text/refine-goal", draggedId);
      e.dataTransfer.effectAllowed = "move";
      requestAnimationFrame(() => el.classList.add("planning-dragging"));
    };
    el.ondragend = () => {
      draggedId = null;
      el.classList.remove("planning-dragging");
      clearDrop();
    };
    el.onkeydown = (e) => {
      if (e.target === el && e.key.toLowerCase() === "m") {
        e.preventDefault();
        planningMove(card(el.dataset.card));
      }
    };
  });
  root.querySelectorAll("[data-lane]").forEach((lane) => {
    const destination = (e) => {
      const peers = [...lane.querySelectorAll("[data-card]")].filter(
        (el) => el.dataset.card !== draggedId,
      );
      const before = peers.find(
        (el) =>
          e.clientY <
          el.getBoundingClientRect().top +
            el.getBoundingClientRect().height / 2,
      );
      return { peers, before };
    };
    lane.ondragover = (e) => {
      if (![...e.dataTransfer.types].includes("text/refine-goal")) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      clearDrop();
      lane.classList.add("planning-drop-lane");
      const { peers, before } = destination(e);
      if (before) before.classList.add("planning-drop-before");
      else peers.at(-1)?.classList.add("planning-drop-after");
      const viewport = root.querySelector(".planning-lanes");
      const bounds = viewport.getBoundingClientRect();
      if (e.clientX < bounds.left + 48) viewport.scrollLeft -= 24;
      if (e.clientX > bounds.right - 48) viewport.scrollLeft += 24;
    };
    lane.ondragleave = (e) => {
      if (!lane.contains(e.relatedTarget)) clearDrop();
    };
    lane.ondrop = run(async (e) => {
      e.preventDefault();
      clearDrop();
      const c = card(e.dataTransfer.getData("text/refine-goal"));
      if (!c) return;
      draggedId = c.placement.goal_id;
      const { peers, before } = destination(e);
      const index = before ? peers.indexOf(before) : peers.length;
      const previous = index
        ? card(peers[index - 1].dataset.card).placement.position
        : null;
      const next = before ? card(before.dataset.card).placement.position : null;
      const position =
        previous == null
          ? (next ?? 1024) - 1024
          : next == null
            ? previous + 1024
            : (previous + next) / 2;
      draggedId = null;
      await planningCommand(
        "card.move",
        { ...fields(c), lane_id: lane.dataset.lane },
        { position },
      );
    });
  });
}

function planningOpenComposer(board, laneId, button) {
  document.querySelectorAll(".planning-composer").forEach((el) => el._close());
  const root = document.createElement("form");
  root.className = "planning-composer";
  const listId = `planning-suggestions-${planningRequestId()}`;
  const hintId = `${listId}-hint`;
  root.innerHTML = `<label class="visually-hidden" for="${listId}-input">Card title or existing Goal</label><input id="${listId}-input" autocomplete="off" placeholder="Write an idea or find a Goal…" role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="${listId}" aria-describedby="${hintId}" maxlength="500"><div id="${listId}" role="listbox" aria-label="Create or choose a card"></div><p class="planning-composer-hint" id="${hintId}">Enter to create · ↑ ↓ to choose · Esc to close</p><p class="form-error" role="status" data-composer-status></p><button type="button" class="subtle" data-composer-close>Cancel</button>`;
  button.hidden = true;
  button.parentElement.before(root);
  const input = root.querySelector("input"),
    list = root.querySelector('[role="listbox"]'),
    status = root.querySelector("[data-composer-status]");
  let timer,
    sequence = 0,
    choices = [],
    active = 0,
    busy = false;
  const generation = captureNodeContextGeneration();
  const submitCreate = planningSubmitter("card.create"),
    submitAttach = planningSubmitter("card.attach"),
    submitMove = planningSubmitter("card.move");
  root._close = () => {
    clearTimeout(timer);
    sequence++;
    root.remove();
    button.hidden = false;
    button.focus();
  };
  root.querySelector("[data-composer-close]").onclick = root._close;
  const draw = () => {
    input.setAttribute("aria-expanded", String(choices.length > 0));
    if (choices.length)
      input.setAttribute("aria-activedescendant", `${listId}-${active}`);
    else input.removeAttribute("aria-activedescendant");
    list.innerHTML = choices
      .map(
        (choice, i) =>
          `<div role="option" id="${listId}-${i}" aria-selected="${active === i}" data-choice="${i}"><strong>${htmlEscape(choice.goal ? choice.goal.name : `Create “${input.value.trim()}”`)}</strong><span>${htmlEscape(choice.subtitle)}</span></div>`,
      )
      .join("");
    list.querySelectorAll("[data-choice]").forEach((option) => {
      option.onmousedown = (e) => e.preventDefault();
      option.onclick = () => choose(Number(option.dataset.choice));
    });
  };
  const choose = async (index) => {
    if (busy || !choices[index]) return;
    if (!isNodeContextGenerationCurrent(generation)) {
      status.textContent =
        "Project or node changed. Reopen Add card before saving.";
      return;
    }
    if (choices[index].openOnly) {
      const choice = choices[index];
      root._close();
      location.hash = `#/planning?board=${encodeURIComponent(choice.placement.board_id)}&card=${encodeURIComponent(choice.goal.id)}`;
      return;
    }
    busy = true;
    input.disabled = true;
    root.querySelector("[data-composer-close]").disabled = true;
    root.setAttribute("aria-busy", "true");
    const choice = choices[index],
      name = input.value.trim();
    status.textContent = choice.goal ? "Adding Goal…" : "Creating card…";
    try {
      const args = { board_id: board.id, lane_id: laneId };
      const action = choice.placement
        ? await submitMove({
            ...args,
            goal_id: choice.goal.id,
            expected_revision: choice.placement.revision,
          })
        : choice.goal
          ? await submitAttach({ ...args, goal_id: choice.goal.id })
          : await submitCreate(args, {
              name,
              description: name,
              reporter: state.lastReporter || "",
            });
      root._close();
      await refreshPlanning();
      if (typeof toast === "function")
        toast(
          action.state === "complete"
            ? "Card added"
            : "Card queued — it will appear when ready.",
          "info",
        );
    } catch (error) {
      status.textContent = error.message;
      input.disabled = false;
      input.focus();
    } finally {
      busy = false;
      root.querySelector("[data-composer-close]").disabled = false;
      root.removeAttribute("aria-busy");
    }
  };
  input.oninput = () => {
    clearTimeout(timer);
    const ticket = ++sequence,
      query = input.value.trim();
    active = 0;
    choices = query
      ? [
          {
            subtitle:
              "New card in " + board.lanes.find((l) => l.id === laneId).name,
          },
        ]
      : [];
    status.textContent = "";
    draw();
    if (!query) return;
    timer = setTimeout(async () => {
      try {
        const result = await api(
          "GET",
          `/api/goals?q=${encodeURIComponent(query)}&node=all&limit=8`,
          undefined,
          { cache: false },
        );
        if (
          ticket !== sequence ||
          !root.isConnected ||
          !isNodeContextGenerationCurrent(generation)
        )
          return;
        const matches = (result.goals || []).flatMap((goal) => {
          const placement = planningSnapshot.cards.find(
            (c) => c.placement.goal_id === goal.id,
          )?.placement;
          const from =
            placement &&
            planningSnapshot.boards.find((b) => b.id === placement.board_id);
          return [
            {
              goal,
              placement,
              openOnly:
                placement?.archived ||
                from?.archived ||
                (placement?.board_id === board.id &&
                  placement.lane_id === laneId),
              subtitle:
                placement?.archived || from?.archived
                  ? "Archived · Open on board"
                  : placement?.board_id === board.id &&
                      placement.lane_id === laneId
                    ? "Already in this lane · Open card"
                    : placement
                      ? `Move from ${from?.name || "another board"}`
                      : `${workflowStatusLabel(goal.status)} · Add existing Goal`,
            },
          ];
        });
        choices = [choices[0], ...matches];
        active = Math.min(active, choices.length - 1);
        draw();
      } catch (error) {
        if (ticket === sequence && root.isConnected)
          status.textContent =
            "Search unavailable. You can still create a card.";
      }
    }, 180);
  };
  input.onkeydown = (e) => {
    if (["ArrowDown", "ArrowUp"].includes(e.key) && choices.length) {
      e.preventDefault();
      active =
        (active + (e.key === "ArrowDown" ? 1 : -1) + choices.length) %
        choices.length;
      draw();
      document
        .getElementById(`${listId}-${active}`)
        ?.scrollIntoView({ block: "nearest" });
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      root._close();
    }
  };
  root.onsubmit = (e) => {
    e.preventDefault();
    choose(active);
  };
  input.focus();
}

function planningMove(card) {
  const p = card.placement,
    esc = htmlEscape;
  const boards = planningSnapshot.boards.filter((b) => !b.archived);
  const root = hubModal(
    "Move card",
    `<label>Board<select data-board>${boards.map((b) => `<option value="${esc(b.id)}" ${b.id === p.board_id ? "selected" : ""}>${esc(b.name)}</option>`).join("")}</select></label><label>Lane<select data-lane></select></label><label>Position<select data-position><option value="bottom">Bottom</option><option value="top">Top</option></select></label><button data-planning-save>Move</button>`,
  );
  const populate = () => {
    const board = boards.find(
      (b) => b.id === root.querySelector("[data-board]").value,
    );
    root.querySelector("[data-lane]").innerHTML = board.lanes
      .map(
        (l) =>
          `<option value="${esc(l.id)}" ${l.id === p.lane_id ? "selected" : ""}>${esc(l.name)}</option>`,
      )
      .join("");
  };
  populate();
  root.querySelector("[data-board]").onchange = populate;
  root.classList.add("planning-editor");
  const submit = planningSubmitter("card.move");
  root.querySelector("[data-planning-save]").onclick = () =>
    hubAction(root, async () => {
      const board_id = root.querySelector("[data-board]").value,
        lane_id = root.querySelector("[data-lane]").value;
      const positions = planningCards(board_id, lane_id).map(
        (c) => c.placement.position,
      );
      const position =
        root.querySelector("[data-position]").value === "top"
          ? Math.min(0, ...positions) - 1024
          : Math.max(Date.now(), ...positions) + 1024;
      await submit(
        {
          goal_id: p.goal_id,
          expected_revision: p.revision,
          board_id,
          lane_id,
        },
        { position },
      );
      root._close();
      await refreshPlanning();
    });
}
function planningEdit(title, operation, fields, values, board) {
  const esc = htmlEscape;
  const input = (key, value) =>
    key === "routing"
      ? `<select name="routing">${[["", "Inherit from lane or board"], ["auto", "Automatic: least loaded node"], ...(planningSnapshot?.nodes || []).filter((n) => !n.archived).map((n) => [n.id, `${n.display_name || n.id}${n.enabled ? "" : " (disabled)"}`]), ...(value && value !== "auto" && !(planningSnapshot?.nodes || []).some((n) => n.id === value) ? [[value, value]] : [])].map(([id, label]) => `<option value="${esc(id)}" ${id === value ? "selected" : ""}>${esc(label)}</option>`).join("")}</select>`
      : key === "action"
        ? `<select name="action">${[
            ["none", "Organize only"],
            ["accept_into_backlog", "Accept into Backlog"],
            ["release", "Release to execution"],
          ]
            .map(
              ([id, label]) =>
                `<option value="${id}" ${id === value ? "selected" : ""}>${label}</option>`,
            )
            .join("")}</select>`
        : key === "priority"
          ? `<select name="priority">${["low", "medium", "high"].map((priority) => `<option value="${priority}" ${priority === value ? "selected" : ""}>${priority[0].toUpperCase() + priority.slice(1)}</option>`).join("")}</select>`
          : key === "description"
            ? `<textarea name="description" rows="6">${esc(value)}</textarea>`
            : `<input name="${key}" value="${esc(value)}" ${key === "name" ? "required" : ""} ${key === "routing" ? 'placeholder="Inherit, auto, or node ID"' : ""}>`;
  const root = hubModal(
    title,
    `<form>${"expected_goal_revision" in values ? `<input type="hidden" name="expected_goal_revision" value="${values.expected_goal_revision}">` : ""}${Object.entries(
      values,
    )
      .filter(([key]) => key !== "expected_goal_revision")
      .map(
        ([key, value]) =>
          `<label class="form-row">${esc({ goal_id: "Goal ID", routing: "Execution routing" }[key] || key[0].toUpperCase() + key.slice(1))}${input(key, value)}</label>`,
      )
      .join(
        "",
      )}<p role="alert" data-error></p><button type="submit">Save</button></form>${operation === "card.update" ? "<button data-detach>Remove from board</button>" : ""}${board && operation === "board.update" ? `<button data-archive>${board.archived ? "Restore" : "Archive"} board</button>` : ""}${board && operation === "lane.update" ? "<button data-lane-left>Move lane left</button><button data-lane-right>Move lane right</button><button data-delete>Delete empty lane</button><button data-skill>Add lane Skill</button>" : ""}`,
  );
  root.classList.add("planning-editor");
  root.querySelector("input:not([type=hidden]), textarea, select")?.focus();
  const submit = planningSubmitter(operation);
  let saving = false;
  root.querySelector("form").onsubmit = async (e) => {
    e.preventDefault();
    if (saving) return;
    if (!isNodeContextGenerationCurrent(root._nodeGeneration)) {
      root.querySelector("[data-error]").textContent =
        "Project or node changed. Reopen this editor before saving.";
      return;
    }
    saving = true;
    const data = Object.fromEntries(new FormData(e.target));
    if ("expected_goal_revision" in data)
      data.expected_goal_revision = Number(data.expected_goal_revision);
    if ("routing" in data) data.routing = data.routing.trim() || null;
    const args = { ...fields };
    if (data.goal_id) {
      args.goal_id = data.goal_id;
      delete data.goal_id;
    }
    try {
      const action = await submit(args, data);
      if (operation === "board.create" && action.result?.id) {
        planningBoardId = action.result.id;
        history.replaceState(
          null,
          "",
          `#/planning?board=${encodeURIComponent(planningBoardId)}`,
        );
      }
      root._close();
      await refreshPlanning();
    } catch (error) {
      root.querySelector("[data-error]").textContent = error.message;
    } finally {
      saving = false;
    }
  };
  root.querySelector("[data-detach]")?.addEventListener("click", () =>
    hubAction(root, async () => {
      await planningCommand("card.detach", fields);
      root._close();
      await refreshPlanning();
    }),
  );
  root.querySelector("[data-archive]")?.addEventListener("click", () =>
    hubAction(root, async () => {
      await planningCommand("board.archive", fields, {
        archived: !board.archived,
      });
      root._close();
      refreshPlanning();
    }),
  );
  for (const [selector, delta] of [
    ["[data-lane-left]", -1],
    ["[data-lane-right]", 1],
  ])
    root.querySelector(selector)?.addEventListener("click", () =>
      hubAction(root, async () => {
        const ids = board.lanes.map((l) => l.id),
          at = ids.indexOf(fields.lane_id),
          to = Math.max(0, Math.min(ids.length - 1, at + delta));
        [ids[at], ids[to]] = [ids[to], ids[at]];
        await planningCommand(
          "lane.reorder",
          { board_id: board.id, expected_revision: board.revision },
          { lane_ids: ids },
        );
        root._close();
        refreshPlanning();
      }),
    );
  root.querySelector("[data-delete]")?.addEventListener("click", () =>
    hubAction(root, async () => {
      await planningCommand("lane.delete", fields);
      root._close();
      refreshPlanning();
    }),
  );
  root.querySelector("[data-skill]")?.addEventListener("click", () => {
    root._close();
    openSkillEditor(null, false, {
      source: "planning.lane.enter",
      planning: { board_id: fields.board_id, lane_id: fields.lane_id },
    });
  });
}
setInterval(() => {
  if (
    state.currentRoute === "planning" &&
    !document.querySelector(".modal-backdrop") &&
    !document.querySelector(".planning-composer") &&
    !document.querySelector(".planning-dragging") &&
    !document.querySelector("#planning-board-menu[open]")
  )
    refreshPlanning();
}, 5000);

let planningNavigationGeneration = 0;
function renderPlanningNavigation(snapshot) {
  const options = document.getElementById("planning-board-options"),
    rail = document.getElementById("rail-planning-boards");
  if (!options || !rail) return;
  const selected =
    new URLSearchParams(location.hash.split("?")[1] || "").get("board") ||
    planningBoardId;
  const boards = (snapshot.boards || []).filter(
    (board) => planningShowArchived || !board.archived,
  );
  const links = boards
    .map(
      (board) =>
        `<a class="rail-row${state.currentRoute === "planning" && board.id === selected ? " active" : ""}" href="#/planning?board=${encodeURIComponent(board.id)}" data-route="planning" data-planning-nav-board="${htmlEscape(board.id)}" title="${htmlEscape(board.name)}"${state.currentRoute === "planning" && board.id === selected ? ' aria-current="page"' : ""}><svg class="rail-icon" aria-hidden="true" viewBox="0 0 24 24"><use href="/static/vendor/lucide/navigation.svg#kanban"></use></svg><span class="rail-copy">${htmlEscape(board.name)}${board.archived ? " (archived)" : ""}</span></a>`,
    )
    .join("");
  rail.innerHTML =
    links ||
    '<a class="rail-row" href="#/planning" data-route="planning"><span class="rail-copy">Create your first board</span></a>';
  options.innerHTML = `<a class="nav-menu-item" href="#/planning" data-route="planning" data-testid="nav-planning">Project Planning</a>${links.replaceAll(" active", "").replaceAll(' aria-current="page"', "")}`;
  options.querySelectorAll("a").forEach((link) => {
    link.onclick = () => {
      document.getElementById("planning-board-menu").open = false;
    };
  });
  if (typeof positionRailMenus === "function") positionRailMenus();
}
async function refreshPlanningNavigation() {
  const ticket = ++planningNavigationGeneration,
    generation = captureNodeContextGeneration();
  const root = document.getElementById("rail-planning-boards");
  if (!root) return;
  try {
    const snapshot = await api("GET", "/api/planning", undefined, {
      cache: false,
      recordError: false,
    });
    if (
      ticket !== planningNavigationGeneration ||
      !isNodeContextGenerationCurrent(generation)
    )
      return;
  } catch (_) {
    if (
      ticket === planningNavigationGeneration &&
      isNodeContextGenerationCurrent(generation)
    )
      renderPlanningNavigation({ boards: [] });
  }
}
document
  .getElementById("planning-board-menu")
  ?.addEventListener("toggle", (event) => {
    if (event.target.open) refreshPlanningNavigation();
  });
document
  .querySelector("[data-planning-create-board]")
  ?.addEventListener("click", async () => {
    document.getElementById("planning-board-menu").open = false;
    if (typeof closeMobileNavigation === "function") closeMobileNavigation();
    try {
      _prevHashURL = location.href;
      history.pushState(null, "", "#/planning");
      await navigate();
      planningEdit("New board", "board.create", {}, { name: "", routing: "" });
    } catch (error) {
      showActionError(error);
    }
  });
window.addEventListener("load", refreshPlanningNavigation);
