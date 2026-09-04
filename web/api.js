// Thin wrappers over the API.
//
// Every call is same-origin: the page is served by the same `agtx serve` that
// answers /api, so there is no base URL to configure and nothing to get wrong
// when the tunnel hostname changes.

const TOKEN_KEY = "agtx.token";

/// Consume a credential from the URL fragment.
///
/// Two forms. `#pair=<code>` is the QR: a single-use code exchanged once for
/// this device's own token. `#token=<t>` is the older direct hand-off, kept so
/// a link from before per-device pairing still works.
///
/// Either way the secret has to leave the address bar immediately — it should
/// not sit in a screenshot, a shared link, or browser history. Fragments are
/// never sent to the server, which is why credentials travel this way rather
/// than in a query parameter.
///
/// Note the collision this avoids: the app's own router lives in the hash
/// (`#/p/...`), so the credential forms are consumed *before* the first route
/// is read, and the hash is replaced with the default route rather than
/// cleared — an empty hash leaves the router nothing to parse.
export async function captureCredential() {
  const hash = location.hash.replace(/^#/, "");

  if (hash.startsWith("token=")) {
    storeToken(decodeURIComponent(hash.slice("token=".length)));
    history.replaceState(null, "", `${location.pathname}#/`);
    return null;
  }

  if (!hash.startsWith("pair=")) return null;
  const code = decodeURIComponent(hash.slice("pair=".length));
  // Replace the hash *before* the exchange: a reload mid-request would
  // otherwise retry a code that is already spent and show a failure for
  // pairing that actually succeeded.
  history.replaceState(null, "", `${location.pathname}#/`);

  try {
    const res = await request("POST", "/api/pair", { code, label: deviceLabel() });
    storeToken(res.token);
    return { paired: res.label };
  } catch (e) {
    return { error: e.message };
  }
}

function storeToken(token) {
  if (!token) return;
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // Private browsing, or storage disabled. The token still works for this
    // page load; it just will not survive a reload.
    memoryToken = token;
  }
}

/// A human-readable name for this device, for the revoke list.
///
/// Deliberately coarse. The list needs "which phone is this" answered well
/// enough to revoke the right row, and a full user-agent string is both longer
/// and worse at saying that.
function deviceLabel() {
  const ua = navigator.userAgent || "";
  const device =
    /iPhone/.test(ua) ? "iPhone"
    : /iPad/.test(ua) ? "iPad"
    : /Android/.test(ua) ? "Android"
    : /Macintosh/.test(ua) ? "Mac"
    : /Windows/.test(ua) ? "Windows"
    : /Linux/.test(ua) ? "Linux"
    : "device";
  const browser =
    /CriOS|Chrome/.test(ua) ? "Chrome"
    : /Firefox/.test(ua) ? "Firefox"
    : /Safari/.test(ua) ? "Safari"
    : null;
  return browser ? `${device} (${browser})` : device;
}

let memoryToken = null;

export function getToken() {
  if (memoryToken) return memoryToken;
  try {
    return localStorage.getItem(TOKEN_KEY) || null;
  } catch {
    return null;
  }
}

async function get(path) {
  return request("GET", path);
}

async function request(method, path, body) {
  const headers = { accept: "application/json" };
  if (body) headers["content-type"] = "application/json";
  const token = getToken();
  if (token) headers.authorization = `Bearer ${token}`;

  const res = await fetch(path, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    // The server answers errors as {"error": "..."} — prefer its wording over
    // a status code, since it says which id was not found.
    let detail = `HTTP ${res.status}`;
    try {
      const body = await res.json();
      if (body && body.error) detail = body.error;
    } catch {
      /* not JSON; the status is all we have */
    }
    if (res.status === 401) {
      // Nothing the user can fix by retrying: the page needs the link agtx
      // printed, and saying so beats an endless row of failed requests.
      const err = new Error(
        "This device is not authorised. Open the link agtx printed at startup — it carries the token.",
      );
      err.status = 401;
      throw err;
    }
    const err = new Error(detail);
    err.status = res.status;
    // 409 means "not yet" rather than "never": the board can move underneath a
    // phone between rendering a button and someone pressing it, and a client
    // should offer a refresh rather than tell the user they did something wrong.
    err.retryable = res.status === 409 || res.status === 429;
    throw err;
  }
  return res.json();
}

export const api = {
  health: () => get("/api/health"),
  projects: () => get("/api/projects"),
  tasks: (pid) => get(`/api/projects/${encodeURIComponent(pid)}/tasks`),
  task: (pid, tid) =>
    get(`/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}`),
  diff: (pid, tid) =>
    get(`/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}/diff`),
  pane: (pid, tid, lines = 200) =>
    get(
      `/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}/pane?lines=${lines}`,
    ),

  // Writes. `action` only *queues*: the board moves when a TUI drains the
  // request, which is what `requestStatus` reports.
  action: (pid, tid, action, reason) =>
    request(
      "POST",
      `/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}/action`,
      { action, reason },
    ),
  requestStatus: (pid, rid) =>
    get(`/api/projects/${encodeURIComponent(pid)}/requests/${encodeURIComponent(rid)}`),
  createTask: (pid, fields) =>
    request("POST", `/api/projects/${encodeURIComponent(pid)}/tasks`, fields),
  updateTask: (pid, tid, fields) =>
    request(
      "PATCH",
      `/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}`,
      fields,
    ),
  input: (pid, tid, payload) =>
    request(
      "POST",
      `/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}/input`,
      payload,
    ),
  deleteTask: (pid, tid) =>
    request(
      "DELETE",
      `/api/projects/${encodeURIComponent(pid)}/tasks/${encodeURIComponent(tid)}`,
    ),
};

/// How each action reads on a button. The API speaks verbs; a person reads
/// destinations, and "Start planning" is what the board's `m` key means.
export const ACTION_LABELS = {
  research: "Start research",
  move_forward: "Move forward",
  move_to_planning: "Start planning",
  move_to_running: "Start running",
  move_to_review: "Move to review",
  move_to_done: "Mark done",
  resume: "Resume",
  escalate_to_user: "Escalate",
};

/// The action a swipe performs — the one thing you would most likely want from
/// a card without opening it. Everything else needs the action sheet.
export const PRIMARY_ACTION = {
  backlog: "move_to_planning",
  planning: "move_forward",
  running: "move_forward",
  review: "move_to_done",
};

/// The board's five columns, in board order. `backlog` carries research, which
/// is why its label is not just "Backlog" — it matches `TaskStatus::display_name`.
export const COLUMNS = [
  { id: "backlog", label: "Backlog" },
  { id: "planning", label: "Planning" },
  { id: "running", label: "Running" },
  { id: "review", label: "Review" },
  { id: "done", label: "Done" },
];

/// Phase glyphs, matching the TUI's vocabulary so the two views read the same.
export const PHASE = {
  working: { glyph: "▶", label: "working" },
  blocked: { glyph: "?", label: "blocked" },
  idle: { glyph: "⏸", label: "idle" },
  ready: { glyph: "✓", label: "ready" },
  exited: { glyph: "✕", label: "exited" },
};

/// A published phase older than this is a snapshot, not a reading: with no TUI
/// running nothing refreshes `task_runtime`. Generous next to the 2s refresh —
/// this marks "nobody is watching", not "the last tick was slow".
export const PHASE_STALE_SECS = 30;

export function isStale(task) {
  return typeof task.phase_age_secs === "number" && task.phase_age_secs > PHASE_STALE_SECS;
}

/// Whether a task is worth surfacing on the projects screen. Blocked outranks
/// the rest: a task waiting on a permission prompt is the one where a reply
/// from a phone actually unblocks something.
export function needsAttention(task) {
  if (task.escalation_note) return true;
  if (isStale(task)) return false;
  return task.phase_status === "blocked" || task.phase_status === "ready";
}

/// The keys agents actually ask for, in the order a thumb wants them.
///
/// A closed set, mirrored by `PaneKey::parse` on the server: forwarding
/// arbitrary key names would let a tap send `C-d`, which is an EOF that ends
/// the agent's session.
export const KEY_CHIPS = [
  { key: "y", label: "y" },
  { key: "n", label: "n" },
  { key: "1", label: "1" },
  { key: "2", label: "2" },
  { key: "3", label: "3" },
  { key: "Enter", label: "⏎" },
  { key: "Escape", label: "esc" },
  { key: "Up", label: "↑" },
  { key: "Down", label: "↓" },
  { key: "C-c", label: "^C" },
];

/// Phases whose pane can be typed into. Matches the server's own rule, so the
/// keyboard toggle is not offered where every keystroke would 409.
export const INPUTTABLE = new Set(["planning", "running"]);

/// The subprotocol that carries the token on a WebSocket handshake.
///
/// A browser cannot set `Authorization` there, and a query parameter would put
/// the credential in every access log. Mirrors `auth::WS_TOKEN_PREFIX`.
export function wsProtocols() {
  const token = getToken();
  return token ? [`agtx.token.${token}`] : [];
}
