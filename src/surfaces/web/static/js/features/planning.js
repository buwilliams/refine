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
    planningSnapshot = snapshot;
    const boards = snapshot.boards.filter(
      (b) => planningShowArchived || !b.archived,
    );
    const requested = new URLSearchParams(
      location.hash.split("?")[1] || "",
    ).get("board");
    let board =
      boards.find((b) => b.id === (requested || planningBoardId)) || boards[0];
    planningBoardId = board?.id || null;
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
    document.getElementById("main").innerHTML =
      `<section class="planning-page" data-testid="planning-page">
      <div class="actions"><h1>Project Planning</h1><span class="spacer"></span><button data-planning-new-board>New board</button><button class="secondary" data-planning-refresh>Refresh</button></div>
      <p class="muted">Shared across project nodes. Lanes organize cards; badges show Goal progress.</p>
      <div class="actions"><label>Board <select data-planning-board aria-label="Board">${boards.map((b) => `<option value="${esc(b.id)}" ${b.id === board?.id ? "selected" : ""}>${esc(b.name)}${b.archived ? " (archived)" : ""}</option>`).join("")}</select></label>
      <label><input type="checkbox" data-planning-archived ${planningShowArchived ? "checked" : ""}> Show archived</label>
      ${board ? '<button class="secondary" data-planning-board-settings>Board settings</button><button data-planning-new-lane>Add lane</button>' : ""}
      ${!snapshot.migration ? '<button class="secondary" data-planning-migrate>Import Todo Lists</button>' : ""}</div>
      ${(snapshot.errors || []).map((error) => `<p role="alert">${esc(error)}</p>`).join("")}
      ${actions.length ? `<section class="planning-actions" aria-label="Planning actions">${actions.map((a) => `<div role="status"><strong>${esc(a.state)}</strong> · ${esc(a.command.operation)} · ${esc(a.message || `Processing on ${a.owner}`)} <button class="secondary" data-planning-action="${esc(a.id)}">Details</button>${!["failed", "cancelled"].includes(a.state) ? `<button class="secondary" data-planning-cancel="${esc(a.id)}">Cancel</button>` : ""}</div>`).join("")}</section>` : ""}
      ${
        board
          ? `<div class="planning-lanes" aria-label="${esc(board.name)}">${board.lanes
              .map(
                (
                  lane,
                ) => `<section class="planning-lane" data-lane="${esc(lane.id)}" aria-label="${esc(lane.name)}">
        <header><h2>${esc(lane.name)}</h2><button class="secondary" data-lane-settings="${esc(lane.id)}" aria-label="Settings for ${esc(lane.name)}">⋯</button></header>
        ${lane.action !== "none" ? `<p class="planning-lane-action">${lane.action === "release" ? "Releases work" : "Accepts into Backlog"}</p>` : ""}
        <div class="planning-card-list">${planningCards(board.id, lane.id)
          .map((card) => planningCardHtml(card, board))
          .join("")}</div>
        <button class="secondary" data-planning-add="${esc(lane.id)}">Add card</button><button class="secondary" data-planning-attach="${esc(lane.id)}">Add existing Goal</button>
      </section>`,
              )
              .join("")}</div>`
          : "<p>Create a board to collect ideas, organize personal tasks, or release work to your nodes.</p>"
      }
      </section>`;
    bindPlanning(board);
  } catch (error) {
    if (state.currentRoute === "planning") showActionError(error);
  }
}
function planningCardHtml(card, board) {
  const p = card.placement,
    goal = card.goal || {},
    esc = htmlEscape;
  return `<article class="planning-card" draggable="true" data-card="${esc(p.goal_id)}" tabindex="0" aria-label="${esc(goal.name || p.goal_id)}">
    <a href="#/goals/${encodeURIComponent(p.goal_id)}">${esc(goal.name || p.goal_id)}</a>
    <span class="badge status-${esc(goal.status || "draft")}">${esc(workflowStatusLabel(goal.status || "draft"))}</span>
    ${goal.description ? `<p>${esc(goal.description.slice(0, 180))}</p>` : ""}
    ${card.error ? `<p role="alert">${esc(card.error)}</p>` : ""}
    <p class="muted">${esc(goal.reporter || "No Reporter")} · ${esc(goal.priority || "low")} priority<br>Current node: ${esc(goal.node_display_name || goal.node_id || "unknown")}</p>
    <div class="actions">${card.error ? `<button class="secondary" data-card-detach="${esc(p.goal_id)}">Remove from board</button>` : `<button class="secondary" data-card-edit="${esc(p.goal_id)}">Edit</button>`}<button class="secondary" data-card-move="${esc(p.goal_id)}">Move</button><button class="secondary" data-card-archive="${esc(p.goal_id)}">${p.archived ? "Restore" : "Archive"}</button></div>
    ${board.lanes.find((l) => l.id === p.lane_id)?.action !== "none" ? `<button class="secondary" data-card-apply="${esc(p.goal_id)}">Apply lane action</button>` : ""}</article>`;
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
  root.querySelector("[data-planning-board]").onchange = (e) => {
    planningBoardId = e.target.value;
    location.hash = `#/planning?board=${encodeURIComponent(planningBoardId)}`;
  };
  root.querySelector("[data-planning-archived]").onchange = (e) => {
    planningShowArchived = e.target.checked;
    refreshPlanning();
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
  root.querySelectorAll("[data-planning-add]").forEach(
    (el) =>
      (el.onclick = () =>
        planningEdit(
          "New card",
          "card.create",
          { board_id: board.id, lane_id: el.dataset.planningAdd },
          {
            name: "",
            description: "",
            reporter: state.lastReporter || "",
            routing: "",
          },
        )),
  );
  root
    .querySelectorAll("[data-planning-attach]")
    .forEach(
      (el) =>
        (el.onclick = () =>
          planningEdit(
            "Add existing Goal",
            "card.attach",
            { board_id: board.id, lane_id: el.dataset.planningAttach },
            { goal_id: "" },
          )),
    );
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
  root.querySelectorAll("[data-card]").forEach((el) => {
    el.ondragstart = (e) => {
      e.dataTransfer.setData("text/refine-goal", el.dataset.card);
      e.dataTransfer.effectAllowed = "move";
    };
    el.onkeydown = (e) => {
      if (e.target === el && e.key.toLowerCase() === "m") {
        e.preventDefault();
        planningMove(card(el.dataset.card));
      }
    };
  });
  root.querySelectorAll("[data-lane]").forEach((el) => {
    el.ondragover = (e) => {
      if ([...e.dataTransfer.types].includes("text/refine-goal")) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }
    };
    el.ondrop = run((e) => {
      e.preventDefault();
      const c = card(e.dataTransfer.getData("text/refine-goal"));
      if (!c) return;
      const before = e.target.closest("[data-card]");
      const peers = planningCards(board.id, el.dataset.lane).filter(
        (item) => item.placement.goal_id !== c.placement.goal_id,
      );
      const index = before
        ? peers.findIndex(
            (item) => item.placement.goal_id === before.dataset.card,
          )
        : -1;
      const position =
        index >= 0
          ? (peers[index].placement.position +
              (index
                ? peers[index - 1].placement.position
                : peers[index].placement.position - 2048)) /
            2
          : Math.max(
              Date.now(),
              ...peers.map((item) => item.placement.position),
            ) + 1024;
      return planningCommand(
        "card.move",
        { ...fields(c), lane_id: el.dataset.lane },
        { position },
      );
    });
  });
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
          `<label>${esc({ goal_id: "Goal ID", routing: "Execution routing" }[key] || key[0].toUpperCase() + key.slice(1))}${input(key, value)}</label>`,
      )
      .join(
        "",
      )}<p role="alert" data-error></p><button type="submit">Save</button></form>${operation === "card.update" ? "<button data-detach>Remove from board</button>" : ""}${board && operation === "board.update" ? `<button data-archive>${board.archived ? "Restore" : "Archive"} board</button>` : ""}${board && operation === "lane.update" ? "<button data-lane-left>Move lane left</button><button data-lane-right>Move lane right</button><button data-delete>Delete empty lane</button><button data-skill>Add lane Skill</button>" : ""}`,
  );
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
      await submit(args, data);
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
    !document.querySelector(".planning-page :focus")
  )
    refreshPlanning();
}, 5000);
