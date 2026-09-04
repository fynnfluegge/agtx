// agtx mobile — read-only board.
//
// No framework and no build step: plain ES modules, served straight out of the
// binary. Routing is hash-based, which means the server needs no SPA fallback
// and a reload of any screen lands where it was.
//
// Rendering is a full re-render of one screen into `#app`. At the size of a
// kanban board that is cheaper than the machinery to avoid it, and it removes
// the whole class of bugs where the DOM and the data disagree.

import {
  api,
  captureCredential,
  wsProtocols,
  ACTION_LABELS,
  COLUMNS,
  INPUTTABLE,
  KEY_CHIPS,
  PHASE,
  PRIMARY_ACTION,
  isStale,
  needsAttention,
} from "./api.js";
import { paintPane } from "./ansi.js";

const app = document.getElementById("app");

/// The board does not poll.
///
/// A timer refetching every couple of seconds spends battery and cellular data
/// to answer a question nobody asked — a kanban board changes on the scale of
/// minutes, and the person holding the phone knows when they care. So the board
/// is fetched when something *happened*: opened, pulled down, returned to, or
/// changed by an action taken here.
///
/// The live pane is the exception and stays on its socket: that is push, not
/// polling, and watching an agent work is exactly the case where waiting for a
/// human to ask would be wrong.

/// The current screen's fetch, re-run on refresh.
let currentRender = null;
/// When it last completed, so staleness is legible rather than implied.
let lastLoaded = null;

// ── tiny DOM helpers ────────────────────────────────────────────────────

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "class") node.className = v;
    else if (k === "onclick") node.addEventListener("click", v);
    else if (k === "text") node.textContent = v;
    else node.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

function mount(...nodes) {
  app.replaceChildren(...nodes.filter(Boolean));
  lastLoaded = Date.now();
  const main = app.querySelector("main");
  if (main) attachPullToRefresh(main);
}

function header(title, sub, onBack) {
  // A visible control as well as the pull gesture. A gesture is the fast path
  // for a thumb, but it is invisible on a desktop browser and undiscoverable
  // on a phone until someone happens to try it.
  const button = el(
    "button",
    {
      class: "refresh",
      "aria-label": "Refresh",
      onclick: async () => {
        button.classList.add("spinning");
        await refresh();
        button.classList.remove("spinning");
      },
    },
    "↻",
  );

  return el(
    "header",
    { class: "hdr" },
    el(
      "div",
      { class: "hdr-row" },
      onBack && el("button", { class: "back", "aria-label": "Back", onclick: onBack }, "‹"),
      el("h1", {}, title, sub ? el("span", { class: "sub", text: sub }) : null),
      button,
    ),
  );
}

function errorBox(e) {
  return el("div", { class: "err" }, e.message || String(e));
}

/// The banner that separates "queued" from "done".
///
/// Nothing drains `transition_requests` without a running TUI, so actions taken
/// here pile up and the board does not move. Saying so is the whole reason the
/// heartbeat exists — a phone that accepts taps and silently does nothing is
/// worse than one that refuses them.
function disconnectedBanner() {
  return el(
    "div",
    { class: "banner" },
    el("b", {}, "No agtx running. "),
    "Actions will queue until a board is open, and phase status is a snapshot.",
  );
}

// ── overlays ────────────────────────────────────────────────────────────
//
// Toasts and sheets live outside `#app` so a re-render — which happens every
// two seconds — cannot tear one down while it is being read or tapped.

const overlay = document.createElement("div");
document.body.append(overlay);

let toastTimer = null;

/// `undo` gets a button; without one the toast just reports.
function toast(message, { bad = false, undo = null, ms = 5000 } = {}) {
  clearTimeout(toastTimer);
  const node = el(
    "div",
    { class: `toast${bad ? " bad" : ""}`, role: "status" },
    el("span", { class: "msg", text: message }),
    undo ? el("button", { onclick: () => { dismissToast(); undo(); } }, "Undo") : null,
  );
  overlay.replaceChildren(node);
  toastTimer = setTimeout(dismissToast, ms);
}

function dismissToast() {
  clearTimeout(toastTimer);
  if (overlay.firstChild?.classList.contains("toast")) overlay.replaceChildren();
}

function closeOverlay() {
  overlay.replaceChildren();
}

/// A bottom sheet. Tapping the scrim closes it, which is what every native
/// sheet does and what a thumb reaches for first.
function sheet(title, ...rows) {
  const scrim = el("div", {
    class: "scrim",
    onclick: (e) => {
      if (e.target === scrim) closeOverlay();
    },
  });
  scrim.append(
    el(
      "div",
      { class: "sheet", role: "dialog", "aria-modal": "true" },
      el("div", { class: "sheet-grip" }),
      el("h2", { text: title }),
      ...rows.flat().filter(Boolean),
      el("button", { class: "act cancel", onclick: closeOverlay }, "Cancel"),
    ),
  );
  overlay.replaceChildren(scrim);
}

/// Queue an action, optimistically.
///
/// The row lands in `transition_requests` and a TUI executes it later, so there
/// is nothing to await: the card is marked pending immediately and the next
/// poll shows the real state. `undo` is offered only while the request is still
/// pending — once a TUI has picked it up there is nothing to take back, and a
/// button that silently does nothing is worse than no button.
async function runAction(pid, task, action) {
  closeOverlay();
  const label = ACTION_LABELS[action] || action;
  // Mark it before the request so the card dims on the tap rather than on the
  // response — the whole point of doing this optimistically.
  pending.set(task.id, { pid, status: task.status, label, rid: null });
  currentRender && currentRender();

  try {
    const res = await api.action(pid, task.id, action);
    pending.set(task.id, { pid, status: task.status, label, rid: res.request_id });
    toast(
      res.tui_connected ? `${label}: queued` : `${label}: queued — no agtx running`,
      { undo: () => cancelAction(pid, task.id, res.request_id) },
    );
  } catch (e) {
    // Roll back: the server refused, so nothing is queued and the card must
    // stop claiming otherwise.
    pending.delete(task.id);
    currentRender && currentRender();
    toast(e.message, { bad: true, ms: 7000 });
  }
}

/// There is no "cancel a transition request" endpoint, so undo is honest about
/// what it can do: it reports whether the request had already been picked up.
/// Inventing a cancel would mean racing the TUI for a row it may already be
/// acting on.
async function cancelAction(pid, tid, rid) {
  try {
    const status = await api.requestStatus(pid, rid);
    if (status.status === "pending") {
      toast("Still queued — cancel it from the desktop board.", { ms: 6000 });
    } else {
      toast("Too late: agtx already ran it.", { ms: 6000 });
    }
  } catch {
    toast("Too late: agtx already ran it.", { ms: 6000 });
  }
}

/// Tasks with a queued action, keyed by task id.
///
/// A `Set` would not be enough: clearing needs to know what the task's status
/// *was* when the action was queued, since "it moved" is the only signal that
/// the TUI executed it.
const pending = new Map();

/// Reconcile optimistic state against a freshly fetched board.
///
/// Two ways a queued action stops being pending, and both need handling. It
/// lands — the task's status changed, so the card should stop being dimmed. Or
/// the TUI tried and failed, which is reported on the request rather than the
/// task: without checking, a transition that errored would leave the card
/// greyed out forever with no explanation anywhere.
async function reconcilePending(pid, tasks) {
  for (const [tid, entry] of [...pending]) {
    if (entry.pid !== pid) continue;
    const task = tasks.find((t) => t.id === tid);
    if (!task) {
      pending.delete(tid);
      continue;
    }
    if (task.status !== entry.status) {
      pending.delete(tid);
      continue;
    }
    try {
      const status = await api.requestStatus(pid, entry.rid);
      if (status.status === "error") {
        pending.delete(tid);
        toast(`${entry.label} failed: ${status.error || "unknown error"}`, {
          bad: true,
          ms: 9000,
        });
      } else if (status.status === "completed") {
        // Executed without moving the task — a no-op transition, or one whose
        // effect is not a status change. Either way it is no longer pending.
        pending.delete(tid);
      }
    } catch {
      // The request was reaped, so it finished over an hour ago.
      pending.delete(tid);
    }
  }
}

function actionSheet(pid, task) {
  const actions = task.allowed_actions || [];
  const primary = PRIMARY_ACTION[task.status];
  const rows = actions.map((a) =>
    el(
      "button",
      {
        class: `act${a === primary ? " primary" : ""}`,
        onclick: () => runAction(pid, task, a),
      },
      ACTION_LABELS[a] || a,
    ),
  );

  if (task.status === "backlog") {
    rows.push(
      el("button", { class: "act", onclick: () => taskForm(pid, task) }, "Edit"),
      el(
        "button",
        { class: "act danger", onclick: () => confirmDelete(pid, task) },
        "Delete",
      ),
    );
  }

  sheet(
    task.title,
    rows.length ? rows : el("div", { class: "empty" }, "Nothing to do from here."),
  );
}

function confirmDelete(pid, task) {
  sheet(
    `Delete “${task.title}”?`,
    el(
      "button",
      {
        class: "act danger",
        onclick: async () => {
          closeOverlay();
          try {
            await api.deleteTask(pid, task.id);
            toast("Task deleted.");
            currentRender && currentRender();
          } catch (e) {
            toast(e.message, { bad: true, ms: 7000 });
          }
        },
      },
      "Delete",
    ),
  );
}

/// Create when `task` is null, edit otherwise. Both are Backlog-only, which the
/// server enforces regardless of what this offers.
function taskForm(pid, task) {
  const title = el("input", { type: "text", value: task?.title || "", maxlength: 120 });
  const description = el("textarea", {}, task?.description || "");

  const save = el(
    "button",
    {
      class: "act primary",
      onclick: async () => {
        const fields = { title: title.value, description: description.value };
        try {
          if (task) await api.updateTask(pid, task.id, fields);
          else await api.createTask(pid, fields);
          closeOverlay();
          toast(task ? "Saved." : "Task created.");
          currentRender && currentRender();
        } catch (e) {
          toast(e.message, { bad: true, ms: 7000 });
        }
      },
    },
    task ? "Save" : "Create",
  );

  sheet(
    task ? "Edit task" : "New task",
    el("div", { class: "field" }, el("label", { text: "Title" }), title),
    el("div", { class: "field" }, el("label", { text: "Description" }), description),
    save,
  );
  // Focus after the sheet is in the DOM, or iOS ignores it and the keyboard
  // never opens.
  requestAnimationFrame(() => title.focus());
}

// ── routing ─────────────────────────────────────────────────────────────

function route() {
  const raw = location.hash.replace(/^#\/?/, "");
  const parts = raw.split("/").filter(Boolean).map(decodeURIComponent);
  if (parts[0] === "p" && parts[1] && parts[2] === "t" && parts[3]) {
    return { screen: "task", pid: parts[1], tid: parts[3] };
  }
  if (parts[0] === "p" && parts[1]) {
    return { screen: "board", pid: parts[1], column: parts[2] || null };
  }
  return { screen: "projects" };
}

function go(hash) {
  location.hash = hash;
}

/// Show `fn`'s screen, and remember it as what a refresh re-runs.
function show(fn) {
  currentRender = fn;
  return fn();
}

/// Re-run the current screen's fetch.
async function refresh() {
  if (!currentRender) return;
  await currentRender();
  lastLoaded = Date.now();
}

/// Coming back to the app refreshes it.
///
/// Not polling — it fires once, on a deliberate act. Returning to a board that
/// is quietly minutes old is the one case where "the user will pull down" is
/// the wrong answer, because they have no way to know they need to.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") refresh();
});

/// How stale the screen is, in words.
function staleness() {
  if (!lastLoaded) return null;
  const secs = Math.round((Date.now() - lastLoaded) / 1000);
  if (secs < 45) return "just now";
  if (secs < 90) return "a minute ago";
  if (secs < 3600) return `${Math.round(secs / 60)} minutes ago`;
  return "over an hour ago";
}

/// Pull down at the top of the page to refresh.
///
/// Two things it must not do, both found by asking what else is scrolling:
///
/// - **Arm mid-scroll.** `main` is a flex child that does not scroll — the
///   *document* does — so checking `node.scrollTop` would read 0 everywhere and
///   turn any downward drag on a long board into a refresh. The check is
///   against the scrolling element.
/// - **Steal the terminal's own scrolling.** A `.term` or `.mono` pane scrolls
///   inside itself, and a drag there is someone reading output, not asking for
///   fresh data. Those are excluded outright rather than by position, because
///   being at the top of a pane is not a request to leave it.
function attachPullToRefresh(node) {
  const THRESHOLD = 64;
  let startY = null;
  let pulled = 0;

  const indicator = el("div", { class: "pull" }, "↓ pull to refresh");
  node.prepend(indicator);

  const atTop = () => (document.scrollingElement?.scrollTop ?? 0) <= 0;

  node.addEventListener(
    "touchstart",
    (e) => {
      const inPane = e.target instanceof Element && e.target.closest(".term, .mono");
      startY = !inPane && atTop() ? e.touches[0].clientY : null;
      pulled = 0;
    },
    { passive: true },
  );

  node.addEventListener(
    "touchmove",
    (e) => {
      if (startY === null) return;
      pulled = Math.max(0, e.touches[0].clientY - startY);
      // Damped, so the indicator trails the finger rather than tracking it —
      // the standard feel, and it makes the threshold reachable without a
      // gesture that runs off the screen.
      indicator.style.height = `${Math.min(pulled / 2, 48)}px`;
      indicator.textContent = pulled >= THRESHOLD ? "↻ release to refresh" : "↓ pull to refresh";
    },
    { passive: true },
  );

  const release = async () => {
    const fired = startY !== null && pulled >= THRESHOLD;
    startY = null;
    pulled = 0;
    indicator.style.height = "";
    if (fired) {
      indicator.textContent = "↻ refreshing…";
      indicator.style.height = "24px";
      await refresh();
    }
  };
  node.addEventListener("touchend", release);
  node.addEventListener("touchcancel", release);
}

// ── screens ─────────────────────────────────────────────────────────────

async function screenProjects() {
  let projects;
  let health;
  try {
    [projects, health] = await Promise.all([api.projects(), api.health()]);
  } catch (e) {
    mount(header("Projects"), el("main", {}, errorBox(e)));
    return;
  }

  // One board fetch per project, so a card can show counts and whether
  // anything wants a human. Fine at the size of a project index; if that stops
  // being true the answer is a summary field on /api/projects, not fewer cards.
  const boards = await Promise.all(
    projects.map((p) =>
      api.tasks(p.id).catch(() => null),
    ),
  );

  const cards = projects.map((p, i) => {
    const tasks = boards[i];
    const active = tasks
      ? tasks.filter((t) => t.status !== "done" && t.status !== "backlog").length
      : null;
    const attention = tasks ? tasks.filter(needsAttention).length : 0;

    return el(
      "button",
      { class: "card", onclick: () => go(`#/p/${encodeURIComponent(p.id)}`) },
      el(
        "div",
        { class: "card-top" },
        el("span", { class: "card-title", text: p.name }),
        attention > 0 ? el("span", { class: "dot", title: "needs attention" }) : null,
      ),
      el(
        "div",
        { class: "card-meta" },
        tasks === null
          ? el("span", { class: "chip", text: "unreachable" })
          : el("span", { class: "chip", text: `${tasks.length} tasks` }),
        active ? el("span", { class: "chip", text: `${active} active` }) : null,
        attention ? el("span", { class: "chip deps", text: `${attention} need you` }) : null,
        p.tui_connected
          ? null
          : el("span", { class: "chip", text: "no agtx running" }),
      ),
    );
  });

  mount(
    header("agtx", `${projects.length} project${projects.length === 1 ? "" : "s"}`),
    el(
      "main",
      {},
      cards.length ? cards : el("div", { class: "empty" }, "No projects indexed yet."),
      el("div", { class: "foot", text: `agtx ${health.version}` }),
    ),
  );
}

async function screenBoard(pid, column) {
  let tasks;
  let projects;
  try {
    [tasks, projects] = await Promise.all([api.tasks(pid), api.projects()]);
  } catch (e) {
    mount(header("Board", null, () => go("#/")), el("main", {}, errorBox(e)));
    return;
  }

  const project = projects.find((p) => p.id === pid);
  await reconcilePending(pid, tasks);
  const counts = Object.fromEntries(
    COLUMNS.map((c) => [c.id, tasks.filter((t) => t.status === c.id).length]),
  );
  // Open on the first column with anything in it, so a board whose work is all
  // in Running does not greet the user with an empty Backlog.
  const active = column || COLUMNS.find((c) => counts[c.id] > 0)?.id || "backlog";
  const shown = tasks.filter((t) => t.status === active);

  const segs = el(
    "div",
    { class: "segs", role: "tablist" },
    COLUMNS.map((c) =>
      el(
        "button",
        {
          class: "seg",
          role: "tab",
          "aria-selected": String(c.id === active),
          onclick: () => go(`#/p/${encodeURIComponent(pid)}/${c.id}`),
        },
        c.label,
        el("span", { class: "n", text: counts[c.id] }),
      ),
    ),
  );

  const head = header(project?.name || "Board", null, () => go("#/"));
  head.append(segs);

  mount(
    head,
    project && !project.tui_connected ? disconnectedBanner() : null,
    el(
      "main",
      {},
      shown.length
        ? shown.map((t) => swipeable(pid, t))
        : el("div", { class: "empty" }, `Nothing in ${active}.`),
      // Said plainly, because nothing refreshes on its own: a board with no
      // clock on it invites being read as live when it is an hour old.
      el("div", { class: "foot", text: `Updated ${staleness() ?? "just now"} · pull to refresh` }),
    ),
    el(
      "button",
      { class: "fab", "aria-label": "New task", onclick: () => taskForm(pid, null) },
      "+",
    ),
  );
}

/// Wrap a card so a rightward drag performs its primary action.
///
/// Two things make this safe to have at all. `touch-action: pan-y` on the card
/// lets the browser own vertical scrolling, so a swipe can never fight the
/// list; and the direction is locked on the first few pixels of movement, so a
/// slightly diagonal scroll does not queue work by accident.
///
/// The gesture is a shortcut, never the only route: everything it does is also
/// on the action sheet, which is reachable by long-press here and by a button
/// on the task screen. A hidden gesture as the sole path to an action is how a
/// feature becomes undiscoverable.
function swipeable(pid, task) {
  const card = taskCard(pid, task);

  // Long-press opens the full sheet. Attached to *every* card, before the
  // swipe wrapper is even considered: a task with no primary action — one
  // whose dependencies have not cleared, say — is exactly the one whose
  // remaining actions are hardest to find, and gating the sheet on having a
  // swipe left those cards with no way to act on them at all.
  //
  // `contextmenu` is what a held touch fires on both iOS and Android, and
  // preventing it also suppresses the text-selection callout over the card.
  card.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    actionSheet(pid, task);
  });

  const action = PRIMARY_ACTION[task.status];
  if (!action || !(task.allowed_actions || []).includes(action)) return card;

  const wrap = el(
    "div",
    { class: "swipe" },
    el("div", { class: "swipe-bg" }, ACTION_LABELS[action] || action),
    card,
  );

  const THRESHOLD = 96;
  let x0 = 0;
  let y0 = 0;
  let dx = 0;
  let axis = null;

  card.addEventListener(
    "touchstart",
    (e) => {
      x0 = e.touches[0].clientX;
      y0 = e.touches[0].clientY;
      dx = 0;
      axis = null;
      card.classList.add("sliding");
      card.classList.remove("settling");
    },
    { passive: true },
  );

  card.addEventListener(
    "touchmove",
    (e) => {
      const mx = e.touches[0].clientX - x0;
      const my = e.touches[0].clientY - y0;
      if (axis === null) {
        if (Math.abs(mx) < 8 && Math.abs(my) < 8) return;
        axis = Math.abs(mx) > Math.abs(my) ? "x" : "y";
      }
      if (axis !== "x") return;
      // Rightward only: a leftward drag is where a delete gesture would live,
      // and half-implementing it invites a mis-swipe into a destructive action.
      dx = Math.max(0, mx);
      card.style.transform = `translateX(${dx}px)`;
    },
    { passive: true },
  );

  const settle = () => {
    card.classList.remove("sliding");
    card.classList.add("settling");
    card.style.transform = "";
    const fired = axis === "x" && dx >= THRESHOLD;
    axis = null;
    dx = 0;
    if (fired) runAction(pid, task, action);
  };
  card.addEventListener("touchend", settle);
  card.addEventListener("touchcancel", settle);

  return wrap;
}

function phaseGlyph(task) {
  const p = PHASE[task.phase_status];
  if (!p) return null;
  const stale = isStale(task);
  return el("span", {
    class: `phase ${task.phase_status}${stale ? " stale" : ""}`,
    text: p.glyph,
    title: stale ? `${p.label} (${task.phase_age_secs}s ago)` : p.label,
  });
}

function taskCard(pid, t) {
  return el(
    "button",
    {
      class: `card${pending.has(t.id) ? " pending" : ""}`,
      onclick: () => go(`#/p/${encodeURIComponent(pid)}/t/${encodeURIComponent(t.id)}`),
    },
    el(
      "div",
      { class: "card-top" },
      phaseGlyph(t),
      el("span", { class: "card-title", text: t.title }),
      t.escalation_note ? el("span", { class: "dot", title: "escalated" }) : null,
    ),
    el(
      "div",
      { class: "card-meta" },
      el("span", { class: "chip agent", text: t.agent }),
      t.plugin ? el("span", { class: "chip", text: t.plugin }) : null,
      t.cycle > 1 ? el("span", { class: "chip", text: `cycle ${t.cycle}` }) : null,
      t.pr_number ? el("span", { class: "chip", text: `#${t.pr_number}` }) : null,
      t.deps_satisfied === false
        ? el("span", { class: "chip deps", text: "blocked on deps" })
        : null,
      // `null` is "not checked yet", which must not read as clean — that is
      // the answer someone might merge on. Only a definite `true` shows.
      t.conflicted === true
        ? el("span", { class: "chip conflict", text: "conflicts" })
        : null,
    ),
  );
}

// ── task detail ─────────────────────────────────────────────────────────

/// Which tab the task screen is on. Deliberately module state rather than the
/// URL: it is a view preference within one task, and putting it in the hash
/// would make the back button walk through tabs before leaving the task.
let taskTab = "details";

/// The task screen currently mounted, so the poll can tell a refresh from a
/// first render.
let mountedTask = null;

async function screenTask(pid, tid) {
  // The terminal tab owns its own updates: frames arrive over the socket, and
  // re-rendering here would blank the pane between frames and — worse — take
  // focus out of the compose field every two seconds, mid-message.
  if (taskTab === "terminal" && mountedTask === `${pid}/${tid}/terminal`) return;

  let task;
  try {
    task = await api.task(pid, tid);
  } catch (e) {
    mount(
      header("Task", null, () => go(`#/p/${encodeURIComponent(pid)}`)),
      el("main", {}, errorBox(e)),
    );
    return;
  }

  const head = header(task.title, `${task.status} · ${task.agent}`, () =>
    go(`#/p/${encodeURIComponent(pid)}`),
  );
  const tabs = ["details", "diff", "terminal"];
  head.append(
    el(
      "div",
      { class: "tabs", role: "tablist" },
      tabs.map((name) =>
        el(
          "button",
          {
            class: "tab",
            role: "tab",
            "aria-selected": String(name === taskTab),
            onclick: () => {
              if (taskTab === "terminal" && name !== "terminal") closeSocket();
              taskTab = name;
              mountedTask = null;
              currentRender && currentRender();
            },
          },
          name[0].toUpperCase() + name.slice(1),
        ),
      ),
    ),
  );

  const body = el("main", {});
  mount(head, body);
  mountedTask = `${pid}/${tid}/${taskTab}`;

  if (taskTab === "details") body.replaceChildren(...detailsBody(pid, task));
  else if (taskTab === "diff") body.replaceChildren(await diffBody(pid, tid));
  else body.replaceChildren(terminalBody(pid, task));
}

function detailsBody(pid, task) {
  const actions = task.allowed_actions || [];
  const primary = PRIMARY_ACTION[task.status];
  const buttons = actions.map((a) =>
    el(
      "button",
      {
        class: `act${a === primary ? " primary" : ""}`,
        onclick: () => runAction(pid, task, a),
      },
      ACTION_LABELS[a] || a,
    ),
  );
  if (task.status === "backlog") {
    buttons.push(
      el("button", { class: "act", onclick: () => taskForm(pid, task) }, "Edit"),
      el(
        "button",
        { class: "act danger", onclick: () => confirmDelete(pid, task) },
        "Delete",
      ),
    );
  }

  const rows = [
    ["Status", task.status],
    ["Phase", task.phase_status ? phaseText(task) : "not observed"],
    ["Agent", task.agent],
    ["Agent state", task.blocked_reason || task.agent_state || "—"],
    ["Plugin", task.plugin || "—"],
    ["Branch", task.branch_name || "—"],
    ["Base", task.base_branch || "—"],
    ["Worktree", task.worktree_path || "—"],
    ["Session", task.session_name || "—"],
    ["Dependencies", task.deps_satisfied ? "satisfied" : "not satisfied"],
    ["Updated", new Date(task.updated_at).toLocaleString()],
  ];

  return [
    task.escalation_note
      ? el("div", { class: "note" }, el("h3", { text: "Escalation" }), task.escalation_note)
      : null,
    ...buttons,
    buttons.length
      ? el("hr", { style: "border:0;border-top:1px solid var(--border);margin:0.9rem 0" })
      : null,
    task.description
      ? el("div", { class: "desc", text: task.description })
      : el("div", { class: "empty" }, "No description."),
    el("hr", { style: "border:0;border-top:1px solid var(--border);margin:0.9rem 0" }),
    el(
      "dl",
      { class: "kv" },
      rows.flatMap(([k, v]) => [el("dt", { text: k }), el("dd", { text: v })]),
    ),
  ].filter(Boolean);
}

function phaseText(task) {
  const p = PHASE[task.phase_status];
  const age = task.phase_age_secs;
  const label = p ? p.label : task.phase_status;
  if (isStale(task)) return `${label} — stale, ${age}s ago`;
  return label;
}

async function diffBody(pid, tid) {
  let diff;
  try {
    diff = await api.diff(pid, tid);
  } catch (e) {
    return errorBox(e);
  }
  if (!diff.patch.trim()) {
    return el("div", { class: "empty" }, `No changes against ${diff.base}.`);
  }

  // One collapsible section per file, closed by default. A phone screen holds
  // maybe forty lines; a flat patch of a dozen files means scrolling past all
  // of them to find the one worth reading. Triage is picking the file, then
  // reading it — so the file list *is* the interface and the hunks are what
  // opens underneath.
  const byFile = splitPatchByFile(diff.patch);
  const stats = statLines(diff.stat);

  const sections = byFile.map(({ path, body }) => {
    const hunks = el("div", { class: "mono", hidden: true }, ...colourDiff(body));
    const stat = stats.get(path);
    const head = el(
      "button",
      {
        class: "file-row",
        "aria-expanded": "false",
        onclick: () => {
          const open = hunks.hidden;
          hunks.hidden = !open;
          head.setAttribute("aria-expanded", String(open));
          head.querySelector(".caret").textContent = open ? "▾" : "▸";
        },
      },
      el("span", { class: "caret", text: "▸" }),
      el("span", { class: "path", text: path }),
      stat ? el("span", { class: "n", text: stat }) : null,
    );
    return el("div", { class: "file" }, head, hunks);
  });

  return el(
    "div",
    {},
    el(
      "div",
      { class: "term-bar" },
      el("span", { class: "chip", text: `vs ${diff.base}` }),
      conflictChip(diff),
      el("span", { class: "chip", text: `${byFile.length} files` }),
    ),
    conflictDetail(diff),
    sections.length
      ? sections
      : el("div", { class: "mono" }, ...colourDiff(diff.patch)),
  );
}

function conflictChip(diff) {
  if (diff.conflicted === true) {
    return el("span", { class: "chip conflict", text: "conflicts with base" });
  }
  if (diff.conflicted === false) {
    return el("span", { class: "chip ok", text: "merges cleanly" });
  }
  return null; // not checked — say nothing rather than imply either answer
}

function conflictDetail(diff) {
  if (diff.conflicted !== true || !diff.conflicting_files?.length) return null;
  return el(
    "div",
    { class: "note" },
    el("h3", { text: "Conflicting files" }),
    el("div", { class: "desc", text: diff.conflicting_files.join("\n") }),
  );
}

/// Split a unified diff into one entry per file.
///
/// Anchored on `diff --git`, which git emits once per file regardless of
/// rename, mode change or binary content — unlike `+++ b/...`, which is absent
/// for a deleted file and misleading for a rename.
function splitPatchByFile(patch) {
  const out = [];
  let current = null;
  for (const line of patch.split("\n")) {
    if (line.startsWith("diff --git ")) {
      if (current) out.push(current);
      // `diff --git a/x b/x` — take the b-side, which is the path after any
      // rename, falling back to the whole line if it is shaped unexpectedly.
      const m = line.match(/ b\/(.*)$/);
      current = { path: m ? m[1] : line.slice("diff --git ".length), body: line + "\n" };
    } else if (current) {
      current.body += line + "\n";
    }
  }
  if (current) out.push(current);
  return out;
}

/// `path => "12 +++---"` from `git diff --stat`, for the counts beside each row.
function statLines(stat) {
  const map = new Map();
  for (const line of (stat || "").trim().split("\n")) {
    const bar = line.indexOf("|");
    if (bar === -1) continue;
    map.set(line.slice(0, bar).trim(), line.slice(bar + 1).trim());
  }
  return map;
}

/// Colour a unified diff without a syntax highlighter: +/- and hunk headers
/// carry nearly all the signal at phone width.
function colourDiff(patch) {
  return patch.split("\n").map((line) => {
    let cls = "";
    if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff ")) {
      cls = "diff-file";
    } else if (line.startsWith("@@")) cls = "diff-hunk";
    else if (line.startsWith("+")) cls = "diff-add";
    else if (line.startsWith("-")) cls = "diff-del";
    return el("span", { class: cls, text: line + "\n" });
  });
}

/// The live pane socket, and the task it is showing.
///
/// One socket for the whole app rather than one per view: a phone shows one
/// pane at a time, and reconnecting on every tab switch would cost a handshake
/// for nothing. Closed when the terminal tab is left, so the server stops
/// capturing — nothing is captured for a task nobody is looking at.
let socket = null;
let socketTask = null;
/// Where the live socket paints, and the last frame it painted.
///
/// The frame is cached because the server only sends what *changed*: an idle
/// agent produces no frames at all, so a freshly built pane node with nothing
/// to paint would sit on "connecting…" indefinitely.
const term = { pane: null, live: null, frame: null };

function closeSocket() {
  if (socket) {
    try {
      socket.send(JSON.stringify({ type: "unsubscribe" }));
    } catch {
      /* already closing */
    }
    socket.close();
  }
  socket = null;
  socketTask = null;
  term.pane = null;
  term.live = null;
  term.frame = null;
}

/// A live terminal. Returns immediately with the shell; frames arrive over the
/// socket and repaint in place, so this is not re-rendered by the poll.
function terminalBody(pid, task) {
  const tid = task.id;
  const pane = el("div", { class: "term" });
  const live = el("span", { class: "live off", text: "○ offline" });
  if (term.frame && socketTask === tid) paintPane(pane, term.frame);
  else pane.textContent = "connecting…";

  let narrow = false;
  const narrowBtn = el(
    "button",
    {
      "aria-pressed": "false",
      onclick: () => {
        narrow = !narrow;
        narrowBtn.setAttribute("aria-pressed", String(narrow));
        pane.classList.toggle("narrow", narrow);
      },
    },
    "Bigger text",
  );

  const bar = el(
    "div",
    { class: "term-bar" },
    live,
    el("span", { class: "chip", text: task.session_name || "no session" }),
    narrowBtn,
  );

  const wrap = el("div", {}, bar, pane);

  // Input, only where the server would accept it: a Backlog or Review task has
  // no composer, and offering a keyboard there produces a 409 per keystroke.
  if (INPUTTABLE.has(task.status)) {
    const send = async (payload, what) => {
      try {
        await api.input(pid, tid, payload);
      } catch (e) {
        toast(`${what}: ${e.message}`, { bad: true, ms: 7000 });
      }
    };

    wrap.append(
      el(
        "div",
        { class: "keys" },
        KEY_CHIPS.map((k) =>
          el(
            "button",
            {
              class: k.key === "C-c" ? "warn" : "",
              onclick: () => send({ key: k.key }, k.label),
            },
            k.label,
          ),
        ),
      ),
    );

    const field = el("input", {
      type: "text",
      placeholder: "Message the agent…",
      enterkeyhint: "send",
      autocapitalize: "off",
      autocorrect: "off",
    });
    const submit = async () => {
      const text = field.value.trim();
      if (!text) return;
      // Clear first: the send takes a second or two — a paste plus the Enter
      // loop that gets past a composer's picker — and a field that stays full
      // invites a second tap that would double the message.
      field.value = "";
      await send({ text }, "message");
    };
    field.addEventListener("keydown", (e) => {
      if (e.key === "Enter") submit();
    });
    wrap.append(
      el(
        "div",
        { class: "compose" },
        field,
        el("button", { onclick: submit }, "Send"),
      ),
    );
  }

  openSocket(pid, tid, pane, live);
  return wrap;
}

function openSocket(pid, tid, pane, live) {
  term.pane = pane;
  term.live = live;
  if (socket && socketTask === tid) {
    // Already watching this pane — the new nodes were repainted from the frame
    // cache above, so there is nothing to re-request.
    if (socket.readyState === WebSocket.OPEN) {
      live.className = "live";
      live.textContent = "● live";
    }
    return;
  }
  closeSocket();
  term.pane = pane;
  term.live = live;

  const url = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
  const ws = new WebSocket(url, wsProtocols());
  socket = ws;
  socketTask = tid;

  ws.addEventListener("open", () => {
    if (socket !== ws) return;
    term.live.className = "live";
    term.live.textContent = "● live";
    ws.send(JSON.stringify({ type: "subscribe", project_id: pid, task_id: tid }));
  });

  ws.addEventListener("message", (e) => {
    if (socket !== ws) return;
    let msg;
    try {
      msg = JSON.parse(e.data);
    } catch {
      return;
    }
    if (msg.type === "frame") {
      term.frame = msg.content;
      if (term.pane) paintPane(term.pane, msg.content);
    } else if (msg.type === "gone") {
      term.live.className = "live off";
      term.live.textContent = "○ pane gone";
    } else if (msg.type === "error") {
      if (term.pane) term.pane.textContent = msg.message;
      term.live.className = "live off";
      term.live.textContent = "○ offline";
    }
  });

  const offline = () => {
    if (socket !== ws || !term.live) return;
    term.live.className = "live off";
    term.live.textContent = "○ offline";
  };
  ws.addEventListener("close", offline);
  ws.addEventListener("error", offline);
}

// ── boot ────────────────────────────────────────────────────────────────

function render() {
  const r = route();
  if (r.screen !== "task") {
    // Nothing is captured for a task nobody is looking at — the socket is what
    // makes the server's capture loop run at all.
    closeSocket();
    mountedTask = null;
  }
  if (r.screen === "projects") show(() => screenProjects());
  else if (r.screen === "board") show(() => screenBoard(r.pid, r.column));
  else show(() => screenTask(r.pid, r.tid));
}

// Before the first route is read: a `#pair=…` or `#token=…` hand-off occupies
// the same fragment the router uses, so it has to be consumed first. Pairing is
// a round trip, so the first render waits for it — otherwise the board's own
// requests race the token into storage and the screen opens on an auth error.
captureCredential().then((result) => {
  if (result?.paired) toast(`Paired as “${result.paired}”.`);
  else if (result?.error) toast(result.error, { bad: true, ms: 9000 });
  render();
});

addEventListener("hashchange", render);

if ("serviceWorker" in navigator) {
  // Failure here is not worth surfacing: it costs the offline shell and
  // nothing else, and it always fails over plain http on a non-loopback host.
  navigator.serviceWorker.register("sw.js").catch(() => {});
}
