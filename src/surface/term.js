// TERM-XTERM-1101: Shibubu の Terminal の頁の糊（`term_document` が vendor/xterm の後ろに入れる）。
// Shibubu へ言うのは `term_message` が通す形だけ: ready・in・bin・size・open・menu・shown・key。Shibubu から来るのは w(base64) と setTheme(theme) だけ。
const say = (o) => window.ipc.postMessage(JSON.stringify(o));
const box = document.getElementById('t');
const term = new Terminal({
  allowProposedApi: true, // unicode11 の幅（全角・絵文字）
  fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace',
  fontSize: SHIBUBU_THEME.fontSize,
  lineHeight: 1.15,
  cursorBlink: true,
  scrollback: 10000,
  macOptionClickForcesSelection: true,
  theme: SHIBUBU_THEME.colors,
});
const fit = new FitAddon.FitAddon();
const search = new SearchAddon.SearchAddon();
term.loadAddon(fit);
term.loadAddon(search);
// link は頁の中で開かない（navigation は about: だけ）: Shibubu が既定の browser で開く
term.loadAddon(new WebLinksAddon.WebLinksAddon((e, uri) => { if (e && e.preventDefault) e.preventDefault(); say({ t: 'open', url: uri }); }));
term.loadAddon(new Unicode11Addon.Unicode11Addon());
term.unicode.activeVersion = '11';
term.open(box);
// WebGL の描き手（Orca と同じ）。作れない・失った時は xterm の DOM の描き手のまま（字は同じ font）
try {
  const gl = new WebglAddon.WebglAddon();
  gl.onContextLoss(() => gl.dispose());
  term.loadAddon(gl);
} catch (_) {}

// 大きさ: 枠に合わせ、替わった時だけ言う
let sent = '';
function refit() {
  try { fit.fit(); } catch (_) {}
  const k = term.cols + 'x' + term.rows;
  if (k !== sent && term.cols > 0 && term.rows > 0) { sent = k; say({ t: 'size', cols: term.cols, rows: term.rows }); }
}
new ResizeObserver(refit).observe(box);

// 打った字（IME の確定・貼り付けも）と mouse の報告
let typedAt = 0;
term.onData((d) => { if (!typedAt) typedAt = performance.now(); say({ t: 'in', d }); });
term.onBinary((d) => say({ t: 'bin', d }));

// Shibubu の出力（byte のまま）。打ってから最初に画面に出た時の ms を言う
function w(b64) {
  const s = atob(b64);
  const u = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) u[i] = s.charCodeAt(i);
  term.write(u, () => {
    if (!typedAt) return;
    const t0 = typedAt;
    typedAt = 0;
    requestAnimationFrame(() => say({ t: 'shown', ms: Math.round(performance.now() - t0) }));
  });
}

function setTheme(T) {
  term.options.theme = T.colors;
  term.options.fontSize = T.fontSize;
  document.documentElement.style.background = T.colors.background;
  document.documentElement.style.setProperty('--find-fg', T.colors.foreground);
  refit();
}

// 右 click: 頁の menu を出さず、Shibubu の menu（grid と同じ）をその所に
box.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  const r = box.getBoundingClientRect();
  say({ t: 'menu', x: e.clientX - r.left, y: e.clientY - r.top, sel: term.getSelection() });
}, true);

// ⌘F の検索の欄（頁の中・右上）
const find = document.getElementById('find');
const q = document.getElementById('q');
const none = document.getElementById('none');
const opts = { caseSensitive: false, decorations: { matchOverviewRuler: '#888', activeMatchColorOverviewRuler: '#fff', matchBackground: '#6b5b00', activeMatchBackground: '#a07800' } };
search.onDidChangeResults((r) => { none.hidden = !(q.value && r.resultCount === 0); });
function openFind() { find.hidden = false; q.focus(); q.select(); }
function closeFind() { find.hidden = true; search.clearDecorations(); none.hidden = true; term.focus(); }
q.addEventListener('input', () => { if (q.value) search.findNext(q.value, { ...opts, incremental: true }); else { search.clearDecorations(); none.hidden = true; } });
q.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { e.preventDefault(); e.shiftKey ? search.findPrevious(q.value, opts) : search.findNext(q.value, opts); }
  else if (e.key === 'Escape') { e.preventDefault(); closeFind(); }
});
document.getElementById('prev').onclick = () => search.findPrevious(q.value, opts);
document.getElementById('next').onclick = () => search.findNext(q.value, opts);
document.getElementById('close').onclick = closeFind;

// 頁が持つ key（⌘C・⌘V・⌘X・⌘A は Edit menu・⌘F は検索）以外の ⌘ と ⌃` は Shibubu の short cut（⌘K・⌘T・⌃` …）へ。⌘Z は送らない（Project の Undo を Terminal から起こさない）
const KEEP = new Set(['KeyC', 'KeyV', 'KeyX', 'KeyA', 'KeyZ']);
function shortcut(e) {
  if (e.type !== 'keydown') return false;
  if (e.metaKey && e.code === 'KeyF') { e.preventDefault(); openFind(); return true; }
  if ((e.metaKey && !KEEP.has(e.code)) || (e.ctrlKey && e.code === 'Backquote')) {
    e.preventDefault();
    say({ t: 'key', code: e.code, command: e.metaKey, ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey });
    return true;
  }
  return false;
}
term.attachCustomKeyEventHandler((e) => !shortcut(e));
document.addEventListener('keydown', (e) => { if (find.contains(e.target)) shortcut(e); });

refit();
say({ t: 'ready' });
