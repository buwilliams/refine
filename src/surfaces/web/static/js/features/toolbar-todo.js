// ---- Toolbar: Todo List -----------------------------------------------------

const todoState = {
  reporter: "",
  lists: [],
  selectedListId: "",
  editingItemId: "",
  loading: false,
  error: "",
  requestSeq: 0,
};

function resetTodoState() {
  todoState.requestSeq += 1;
  todoState.reporter = "";
  todoState.lists = [];
  todoState.selectedListId = "";
  todoState.editingItemId = "";
  todoState.loading = false;
  todoState.error = "";
}

function handleTodoReporterChanged(reporter) {
  todoState.requestSeq += 1;
  todoState.reporter = "";
  todoState.lists = [];
  todoState.selectedListId = "";
  todoState.editingItemId = "";
  todoState.loading = false;
  todoState.error = "";
  const todoOpen = Object.values(chatState.tabs).some((tab) => tab.mode === "todo");
  if (!todoOpen) return;
  drawToolbar();
  if (reporter && currentToolbarTab()?.mode === "todo") {
    void loadTodoListsForReporter(reporter);
  }
}

function activeTodoList() {
  return todoState.lists.find((list) => list.id === todoState.selectedListId) || null;
}

function todoListCounts(list) {
  const items = Array.isArray(list?.items) ? list.items : [];
  const completed = items.filter((item) => item.done).length;
  return {
    completed,
    open: items.length - completed,
    total: items.length,
  };
}

function applyTodoResponse(response, preferredListId = "") {
  todoState.reporter = String(response?.reporter || state.lastReporter || "");
  todoState.lists = Array.isArray(response?.lists) ? response.lists : [];
  const candidates = [preferredListId, todoState.selectedListId, response?.list?.id]
    .map((value) => String(value || ""))
    .filter(Boolean);
  todoState.selectedListId = candidates.find((id) =>
    todoState.lists.some((list) => list.id === id)
  ) || todoState.lists[0]?.id || "";
  todoState.editingItemId = "";
  todoState.error = "";
}

async function loadTodoListsForReporter(reporter = state.lastReporter) {
  reporter = String(reporter || "").trim();
  const requestSeq = ++todoState.requestSeq;
  if (!reporter) {
    todoState.reporter = "";
    todoState.lists = [];
    todoState.selectedListId = "";
    todoState.editingItemId = "";
    todoState.loading = false;
    todoState.error = "";
    drawToolbar();
    return;
  }
  todoState.reporter = reporter;
  todoState.loading = true;
  todoState.error = "";
  drawToolbar();
  try {
    const params = new URLSearchParams({ reporter });
    const response = await api("GET", `/api/todos?${params.toString()}`, undefined, { cache: false });
    if (requestSeq !== todoState.requestSeq || state.lastReporter !== reporter) return;
    applyTodoResponse(response);
  } catch (error) {
    if (requestSeq !== todoState.requestSeq || state.lastReporter !== reporter) return;
    todoState.error = error.message || "Could not load todo lists";
  } finally {
    if (requestSeq === todoState.requestSeq && state.lastReporter === reporter) {
      todoState.loading = false;
      drawToolbar();
    }
  }
}

async function runTodoMutation(method, path, body, preferredListId = "") {
  const reporter = state.lastReporter || "";
  if (!reporter) throw new Error("Pick a Reporter before changing todo lists");
  const response = await api(method, path, { reporter, ...body });
  if (state.lastReporter !== reporter) return response;
  applyTodoResponse(response, preferredListId);
  drawToolbar();
  return response;
}

async function createTodoList(name) {
  const response = await runTodoMutation("POST", "/api/todos/lists", { name });
  if (response?.list?.id) {
    applyTodoResponse(response, response.list.id);
    drawToolbar();
  }
  return response;
}

async function renameTodoList(listId, name) {
  return runTodoMutation(
    "PATCH",
    `/api/todos/lists/${encodeURIComponent(listId)}`,
    { name },
    listId,
  );
}

async function deleteTodoList(listId) {
  return runTodoMutation(
    "DELETE",
    `/api/todos/lists/${encodeURIComponent(listId)}`,
    {},
  );
}

async function addTodoItem(listId, text) {
  return runTodoMutation(
    "POST",
    `/api/todos/lists/${encodeURIComponent(listId)}/items`,
    { text },
    listId,
  );
}

async function updateTodoItem(listId, itemId, update) {
  return runTodoMutation(
    "PATCH",
    `/api/todos/lists/${encodeURIComponent(listId)}/items/${encodeURIComponent(itemId)}`,
    update,
    listId,
  );
}

async function deleteTodoItem(listId, itemId) {
  return runTodoMutation(
    "DELETE",
    `/api/todos/lists/${encodeURIComponent(listId)}/items/${encodeURIComponent(itemId)}`,
    {},
    listId,
  );
}

function renderTodoListRail(reporter) {
  return `
    <aside class="todo-list-rail" aria-label="Todo lists">
      <div class="todo-list-rail-header">
        <h3>Lists</h3>
        <button type="button" class="todo-new-list-button" data-todo-new-list
                data-testid="todo-new-list" aria-label="Create a new todo list"
                title="New list">+</button>
      </div>
      <div class="todo-list-nav" data-testid="todo-list-nav">
        ${todoState.lists.map((candidate) => {
          const counts = todoListCounts(candidate);
          return `
            <button type="button"
                    class="todo-list-nav-item${candidate.id === todoState.selectedListId ? " active" : ""}"
                    data-todo-list-id="${htmlEscape(candidate.id)}"
                    data-testid="todo-list-option"
                    aria-pressed="${candidate.id === todoState.selectedListId ? "true" : "false"}">
              <span class="todo-list-nav-name">${htmlEscape(candidate.name)}</span>
              <span class="todo-list-nav-count" aria-label="${counts.open} open">${counts.open}</span>
            </button>`;
        }).join("")}
      </div>
      <div class="todo-list-rail-footer">
        <span class="todo-reporter">${htmlEscape(reporter)}</span>
        <span>Synced across nodes</span>
        <button type="button" class="todo-refresh-button" data-todo-refresh
                data-testid="todo-refresh" ${todoState.loading ? "disabled" : ""}
                aria-label="Refresh todo lists" title="Refresh">↻</button>
      </div>
    </aside>`;
}

function renderTodoItem(item) {
  const editing = todoState.editingItemId === item.id;
  return `
    <li class="todo-item${item.done ? " done" : ""}"
        data-todo-item-id="${htmlEscape(item.id)}">
      <button type="button" class="todo-done-toggle"
              data-todo-toggle data-testid="todo-item-toggle"
              aria-label="${item.done ? "Mark incomplete" : "Mark complete"}: ${htmlEscape(item.text)}"
              aria-pressed="${item.done ? "true" : "false"}">
        <span aria-hidden="true">✓</span>
      </button>
      ${editing ? `
        <form class="todo-edit-form" data-todo-edit-form>
          <label class="sr-only" for="todo-edit-${htmlEscape(item.id)}">Edit todo</label>
          <input id="todo-edit-${htmlEscape(item.id)}" type="text" maxlength="4000"
                 value="${htmlEscape(item.text)}" data-todo-edit-text required>
          <button type="submit" class="small">Save</button>
          <button type="button" class="secondary small" data-todo-edit-cancel>Cancel</button>
        </form>
      ` : `
        <span class="todo-item-text">${htmlEscape(item.text)}</span>
        <span class="todo-item-actions">
          <button type="button" class="subtle small" data-todo-edit
                  data-testid="todo-item-edit" aria-label="Edit ${htmlEscape(item.text)}">Edit</button>
          <button type="button" class="subtle small danger" data-todo-delete
                  data-testid="todo-item-delete" aria-label="Delete ${htmlEscape(item.text)}">Delete</button>
        </span>
      `}
    </li>`;
}

function renderTodoItems(items) {
  const openItems = items.filter((item) => !item.done);
  const completedItems = items.filter((item) => item.done);
  if (!items.length) {
    return `
      <div class="todo-empty" data-testid="todo-empty">
        <span class="todo-empty-mark" aria-hidden="true">✓</span>
        <strong>Nothing here yet</strong>
        <span>Add your first todo above.</span>
      </div>`;
  }
  return `
    ${openItems.length ? `
      <ul class="todo-items" data-testid="todo-items">
        ${openItems.map(renderTodoItem).join("")}
      </ul>
    ` : `
      <div class="todo-all-done" data-testid="todo-all-done">Everything on this list is done.</div>
    `}
    ${completedItems.length ? `
      <section class="todo-completed-section" aria-label="Completed todos">
        <h4>Completed <span>${completedItems.length}</span></h4>
        <ul class="todo-items todo-items-completed">
          ${completedItems.map(renderTodoItem).join("")}
        </ul>
      </section>
    ` : ""}`;
}

function renderTodoWorkspace(list) {
  if (!list) {
    return `
      <main class="todo-workspace todo-workspace-empty" data-testid="todo-no-lists">
        <div class="todo-empty">
          <span class="todo-empty-mark" aria-hidden="true">+</span>
          <strong>Create your first list</strong>
          <span>Keep related todos together and within reach.</span>
          <button type="button" data-todo-new-list>Create a list</button>
        </div>
      </main>`;
  }
  const items = Array.isArray(list.items) ? list.items : [];
  const counts = todoListCounts(list);
  const summary = counts.open === 1 ? "1 todo left" : `${counts.open} todos left`;
  return `
    <main class="todo-workspace">
      <header class="todo-workspace-header">
        <div>
          <h3 data-testid="todo-list-title">${htmlEscape(list.name)}</h3>
          <p>${summary}${counts.completed ? ` · ${counts.completed} completed` : ""}</p>
        </div>
        <details class="todo-list-menu">
          <summary class="todo-list-menu-toggle" data-testid="todo-list-menu-toggle"
                   aria-label="List options" title="List options">•••</summary>
          <div class="todo-list-menu-panel">
            <form data-todo-list-name-form>
              <label for="todo-list-name">List name</label>
              <input id="todo-list-name" type="text" value="${htmlEscape(list.name)}"
                     maxlength="120" data-todo-list-name data-testid="todo-list-name" required>
              <button type="submit" class="secondary" data-testid="todo-list-rename">Rename list</button>
            </form>
            <button type="button" class="danger secondary" data-todo-delete-list
                    data-testid="todo-delete-list">Delete list</button>
          </div>
        </details>
      </header>
      <form class="todo-add-form" data-todo-add-form>
        <label class="sr-only" for="todo-item-text">New todo</label>
        <input id="todo-item-text" type="text" maxlength="4000" autocomplete="off"
               placeholder="What needs to be done?" data-todo-item-text
               data-testid="todo-item-text" required>
        <button type="submit" data-testid="todo-add-item">Add todo</button>
      </form>
      <div class="todo-item-scroll">
        ${renderTodoItems(items)}
      </div>
    </main>`;
}

function renderTodoPanel() {
  const reporter = state.lastReporter || "";
  if (!reporter) {
    return `
      <section class="todo-panel todo-panel-no-reporter" data-testid="toolbar-todo-panel">
        <div class="todo-empty">
          <strong>Choose a Reporter</strong>
          <span>Todo lists belong to the Reporter selected in Controls.</span>
        </div>
      </section>`;
  }

  const list = activeTodoList();
  return `
    <section class="todo-panel" data-testid="toolbar-todo-panel">
      ${todoState.error ? `
        <div class="todo-message error" role="alert" data-testid="todo-error">
          <span>${htmlEscape(todoState.error)}</span>
          <button type="button" class="secondary small" data-todo-refresh>Try again</button>
        </div>
      ` : ""}
      ${todoState.loading && todoState.reporter === reporter && !todoState.lists.length ? `
        <div class="todo-loading" data-testid="todo-loading">Loading todo lists…</div>
      ` : `
        <div class="todo-layout">
          ${renderTodoListRail(reporter)}
          ${renderTodoWorkspace(list)}
        </div>
      `}
    </section>`;
}

function focusTodoComposer(root) {
  requestAnimationFrame(() => root.querySelector("[data-todo-item-text]")?.focus());
}

function bindTodoPanel(root) {
  $$("[data-todo-refresh]", root).forEach((button) => {
    bindOnce(button, "click", () => void loadTodoListsForReporter(state.lastReporter));
  });
  $$("[data-todo-list-id]", root).forEach((button) => {
    bindOnce(button, "click", () => {
      todoState.selectedListId = button.dataset.todoListId || "";
      todoState.editingItemId = "";
      drawToolbar();
      focusTodoComposer($("#toolbar-dock"));
    });
  });
  $$("[data-todo-new-list]", root).forEach((button) => {
    bindOnce(button, "click", async () => {
      const name = await modalPrompt("List name", "", { title: "New todo list" });
      if (!name?.trim()) return;
      try {
        await createTodoList(name.trim());
        focusTodoComposer($("#toolbar-dock"));
      } catch (error) {
        await showActionError(error, "Could not create todo list");
      }
    });
  });

  const list = activeTodoList();
  if (!list) return;
  bindOnce(root.querySelector("[data-todo-list-name-form]"), "submit", async (event) => {
    event.preventDefault();
    const name = root.querySelector("[data-todo-list-name]")?.value.trim() || "";
    if (!name || name === list.name) return;
    try {
      await renameTodoList(list.id, name);
    } catch (error) {
      await showActionError(error, "Could not rename todo list");
    }
  });
  bindOnce(root.querySelector("[data-todo-delete-list]"), "click", async () => {
    const ok = await modalConfirm(`Delete the "${list.name}" todo list and all of its items?`, {
      title: "Delete todo list",
      okLabel: "Delete",
      danger: true,
    });
    if (!ok) return;
    try {
      await deleteTodoList(list.id);
    } catch (error) {
      await showActionError(error, "Could not delete todo list");
    }
  });
  bindOnce(root.querySelector("[data-todo-add-form]"), "submit", async (event) => {
    event.preventDefault();
    const input = root.querySelector("[data-todo-item-text]");
    const text = input?.value.trim() || "";
    if (!text) return;
    try {
      await addTodoItem(list.id, text);
      focusTodoComposer($("#toolbar-dock"));
    } catch (error) {
      await showActionError(error, "Could not add todo");
    }
  });
  $$("[data-todo-item-id]", root).forEach((row) => {
    const items = Array.isArray(list.items) ? list.items : [];
    const item = items.find((candidate) => candidate.id === row.dataset.todoItemId);
    if (!item) return;
    bindOnce(row.querySelector("[data-todo-toggle]"), "click", async () => {
      try {
        await updateTodoItem(list.id, item.id, { done: !item.done });
      } catch (error) {
        await showActionError(error, item.done ? "Could not undo todo" : "Could not complete todo");
      }
    });
    bindOnce(row.querySelector("[data-todo-edit]"), "click", () => {
      todoState.editingItemId = item.id;
      drawToolbar();
      requestAnimationFrame(() => {
        const input = $("#toolbar-dock")?.querySelector("[data-todo-edit-text]");
        input?.focus();
        input?.select();
      });
    });
    bindOnce(row.querySelector("[data-todo-edit-cancel]"), "click", () => {
      todoState.editingItemId = "";
      drawToolbar();
    });
    bindOnce(row.querySelector("[data-todo-edit-form]"), "submit", async (event) => {
      event.preventDefault();
      const text = row.querySelector("[data-todo-edit-text]")?.value.trim() || "";
      if (!text) return;
      if (text === item.text) {
        todoState.editingItemId = "";
        drawToolbar();
        return;
      }
      try {
        await updateTodoItem(list.id, item.id, { text });
      } catch (error) {
        await showActionError(error, "Could not edit todo");
      }
    });
    bindOnce(row.querySelector("[data-todo-delete]"), "click", async () => {
      const ok = await modalConfirm(`Delete "${item.text}"?`, {
        title: "Delete todo",
        okLabel: "Delete",
        danger: true,
      });
      if (!ok) return;
      try {
        await deleteTodoItem(list.id, item.id);
      } catch (error) {
        await showActionError(error, "Could not delete todo");
      }
    });
  });
}
