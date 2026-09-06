// Render a tmux pane capture as HTML.
//
// This is deliberately *not* a terminal emulator, and does not need to be.
// Measured against tmux 3.5a: `capture-pane -e -p` emits only `ESC[…m` — SGR
// colour and attributes. No cursor motion, no scroll regions, no alternate
// screen, because the capture is a snapshot of an already-rendered grid rather
// than a stream of terminal control. There is nothing here for an emulator to
// emulate, so ~150 lines replaces a 250KB dependency and keeps the app free of
// any npm build step (see `src/web/assets.rs`).
//
// If agtx ever grows a real PTY-backed terminal — deferred in the mobile plan —
// that is the point at which xterm.js earns its place, and this file is what it
// would replace.

/// xterm's standard 16, in the agtx palette's register so a pane sits in the
/// same picture as the board around it.
const BASE = [
  "#2a3140", "#ff8b7a", "#7ee787", "#ead49a", "#a0d2fa", "#d3a4f9", "#5cfff7", "#c8c3bc",
  "#5b6273", "#ffab9d", "#a2f0a9", "#f2e4bd", "#c2e2ff", "#e4c6ff", "#a5fff9", "#f2ece6",
];

/// The xterm 256-colour cube and greyscale ramp, computed rather than tabulated.
function xterm256(n) {
  if (n < 16) return BASE[n];
  if (n < 232) {
    const i = n - 16;
    const step = (v) => (v === 0 ? 0 : 55 + v * 40);
    const r = step(Math.floor(i / 36) % 6);
    const g = step(Math.floor(i / 6) % 6);
    const b = step(i % 6);
    return `rgb(${r},${g},${b})`;
  }
  const v = 8 + (n - 232) * 10;
  return `rgb(${v},${v},${v})`;
}

function blankState() {
  return { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };
}

/// Apply one SGR sequence's parameters to `st`.
function applySgr(st, params) {
  for (let i = 0; i < params.length; i++) {
    const p = params[i];
    switch (p) {
      case 0: Object.assign(st, blankState()); break;
      case 1: st.bold = true; break;
      case 2: st.dim = true; break;
      case 3: st.italic = true; break;
      case 4: st.underline = true; break;
      case 7: st.inverse = true; break;
      case 22: st.bold = false; st.dim = false; break;
      case 23: st.italic = false; break;
      case 24: st.underline = false; break;
      case 27: st.inverse = false; break;
      case 39: st.fg = null; break;
      case 49: st.bg = null; break;
      // Extended colour: `38;5;n` (256) or `38;2;r;g;b` (truecolor). The
      // sub-parameters are consumed here so they are not read as further SGR
      // codes — treating `38;5;1` as "38, then 5, then 1" would set blink and
      // a red foreground instead of colour 1.
      case 38:
      case 48: {
        const target = p === 38 ? "fg" : "bg";
        if (params[i + 1] === 5) {
          st[target] = xterm256(params[i + 2] ?? 0);
          i += 2;
        } else if (params[i + 1] === 2) {
          const [r, g, b] = [params[i + 2] ?? 0, params[i + 3] ?? 0, params[i + 4] ?? 0];
          st[target] = `rgb(${r},${g},${b})`;
          i += 4;
        }
        break;
      }
      default:
        if (p >= 30 && p <= 37) st.fg = BASE[p - 30];
        else if (p >= 90 && p <= 97) st.fg = BASE[p - 90 + 8];
        else if (p >= 40 && p <= 47) st.bg = BASE[p - 40];
        else if (p >= 100 && p <= 107) st.bg = BASE[p - 100 + 8];
        break;
    }
  }
}

function styleFor(st) {
  let fg = st.fg;
  let bg = st.bg;
  if (st.inverse) {
    // Swapped against the pane's own defaults, not against nothing: an inverse
    // run with no explicit colours is how a selection or a status bar is drawn,
    // and leaving it unstyled makes it vanish.
    [fg, bg] = [bg || "var(--surface)", fg || "var(--text)"];
  }
  const bits = [];
  if (fg) bits.push(`color:${fg}`);
  if (bg) bits.push(`background:${bg}`);
  if (st.bold) bits.push("font-weight:700");
  if (st.dim) bits.push("opacity:.65");
  if (st.italic) bits.push("font-style:italic");
  if (st.underline) bits.push("text-decoration:underline");
  return bits.join(";");
}

const SGR = /\x1b\[([0-9;]*)m/g;

/// Parse `text` into a document fragment of styled spans.
///
/// Returns a fragment rather than an HTML string so the caller never has to
/// think about escaping: pane content is agent output, which routinely contains
/// `<` and `&`, and one missed escape here would be an injection from anything
/// the agent happens to print.
export function renderAnsi(text) {
  const frag = document.createDocumentFragment();
  const st = blankState();
  let last = 0;

  const emit = (chunk) => {
    if (!chunk) return;
    const style = styleFor(st);
    if (!style) {
      frag.append(document.createTextNode(chunk));
      return;
    }
    const span = document.createElement("span");
    span.setAttribute("style", style);
    span.textContent = chunk;
    frag.append(span);
  };

  SGR.lastIndex = 0;
  let m;
  while ((m = SGR.exec(text)) !== null) {
    emit(text.slice(last, m.index));
    // An empty parameter list means SGR 0 — `ESC[m` is a reset.
    const params = m[1] === "" ? [0] : m[1].split(";").map((n) => parseInt(n, 10) || 0);
    applySgr(st, params);
    last = m.index + m[0].length;
  }
  emit(text.slice(last));

  // Any other escape that slipped through would render as mojibake. Strip the
  // C0 controls that are not newline or tab, since a pane capture should not
  // contain them and showing them as glyphs looks like corruption.
  return frag;
}

/// Replace `node`'s contents with `text` rendered.
///
/// A full replace per frame is the whole synchronisation story: every frame is
/// a complete pane, so there is no incremental state that can drift out of step
/// with the server.
export function paintPane(node, text) {
  // eslint-disable-next-line no-control-regex
  const cleaned = text.replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "");
  node.replaceChildren(renderAnsi(cleaned));
}
