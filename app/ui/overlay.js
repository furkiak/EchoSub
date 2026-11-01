(() => {
  const T = window.__TAURI__;
  const { invoke } = T.core;
  const { listen } = T.event;
  const win = T.window.getCurrentWindow();

  const caps = document.getElementById('caps');
  const statusEl = document.getElementById('status');
  let S = null;            // ayarlar
  let entries = [];        // {seg, text, orig, final, ts}
  let statusTimer = null;

  // ---------------------------------------------------------------- stil
  function applyStyle() {
    if (!S) return;
    const root = document.getElementById('root');
    const a = Math.max(0, Math.min(1, S.bg_opacity));
    const hex = S.bg_color.replace('#', '');
    const r = parseInt(hex.slice(0, 2), 16), g = parseInt(hex.slice(2, 4), 16), b = parseInt(hex.slice(4, 6), 16);
    root.style.background = `rgba(${r},${g},${b},${a})`;
    root.style.borderRadius = a > 0 ? '10px' : '0';
    caps.style.fontFamily = S.font_family;
    caps.style.fontWeight = S.bold ? '700' : '400';
    caps.style.color = S.text_color;
    const w = S.outline_width;
    if (w > 0) {
      caps.style.webkitTextStroke = `${w * 2}px ${S.outline_color}`;
      caps.style.paintOrder = 'stroke fill';
      caps.style.textShadow = `0 2px ${w * 2}px ${S.outline_color}`;
    } else {
      caps.style.webkitTextStroke = '0';
      caps.style.textShadow = 'none';
    }
    fit();
  }

  // Pencereye göre en büyük sığan yazı boyutu; manuel modda sabit px.
  function fit() {
    if (!S) return;
    const H = caps.clientHeight, W = caps.clientWidth;
    if (H <= 0 || W <= 0) return;
    if (!S.auto_fit) {
      caps.style.fontSize = (28 * S.font_scale) + 'px';
      return;
    }
    let lo = 8, hi = Math.max(10, Math.min(H / Math.max(1, Math.min(S.lines, 2)) * 0.8, 220));
    // Alta hizalı (flex-end) kutuda üste taşan içerik scrollHeight'a girmez; yüksekliği çocuklardan topla.
    const used = () => Array.from(caps.children).reduce((a, c) => a + c.offsetHeight, 0);
    for (let i = 0; i < 12; i++) {
      const mid = (lo + hi) / 2;
      caps.style.fontSize = mid + 'px';
      if (used() <= H + 1 && caps.scrollWidth <= W + 1) lo = mid; else hi = mid;
    }
    caps.style.fontSize = (lo * Math.max(0.3, Math.min(1, S.font_scale))) + 'px';
  }

  // ---------------------------------------------------------------- altyazı
  function render() {
    caps.textContent = '';
    for (const e of entries) {
      const d = document.createElement('div');
      d.className = 'line';
      d.textContent = e.text;
      if (S && S.show_original && e.orig && e.orig !== e.text) {
        const o = document.createElement('div');
        o.className = 'orig';
        o.textContent = e.orig;
        d.appendChild(o);
      }
      caps.appendChild(d);
    }
    fit();
  }

  function onCaption(c) {
    const i = entries.findIndex(e => e.seg === c.seg_id);
    if (c.clear) {
      if (i >= 0) entries.splice(i, 1);
      render();
      return;
    }
    const text = c.translated || c.original;
    const ent = { seg: c.seg_id, text, orig: c.original, final: c.final, ts: performance.now() };
    if (i >= 0) entries[i] = ent; else entries.push(ent);
    entries.sort((a, b) => a.seg - b.seg);
    const max = S ? Math.max(1, S.lines) : 2;
    while (entries.length > max) entries.shift();
    render();
  }

  setInterval(() => {
    if (!S || !entries.length) return;
    const lim = S.hide_after_secs * 1000;
    const keep = entries.filter(e => performance.now() - e.ts < lim);
    if (keep.length !== entries.length) { entries = keep; render(); }
  }, 300);

  // ---------------------------------------------------------------- durum
  function showStatus(s) {
    clearTimeout(statusTimer);
    statusEl.textContent = s.msg;
    statusEl.className = 'show ' + (s.stage === 'error' ? 'error' : s.stage === 'warn' ? 'warn' : '');
    if (s.stage === 'ready') statusTimer = setTimeout(() => statusEl.className = '', 2500);
    else if (s.stage === 'warn') statusTimer = setTimeout(() => statusEl.className = '', 8000);
  }

  // ---------------------------------------------------------------- etkileşim
  document.getElementById('gear').addEventListener('click', () => invoke('open_settings'));
  document.getElementById('close').addEventListener('click', () => invoke('quit'));
  for (const g of document.querySelectorAll('.g')) {
    g.addEventListener('mousedown', ev => { ev.preventDefault(); win.startResizeDragging(g.dataset.d); });
  }
  // Üst şeritten sürükle. Tıklama geçirme kapalıysa pencerenin her yerinden de sürüklenir.
  document.getElementById('root').addEventListener('mousedown', ev => {
    if (ev.button !== 0 || ev.target.closest('button') || ev.target.closest('.g')) return;
    if (ev.target.closest('#bar') || noTrack || (S && !S.click_through)) win.startDragging();
  });
  // İmleç konumu okunamıyorsa (Wayland vb.) Rust tıklama geçirmeyi kapatır; arayüz CSS :hover'a döner.
  let noTrack = false;
  listen('tracking', ev => { noTrack = !ev.payload; document.body.classList.toggle('notrack', noTrack); });
  // Rust imleç konumunu izler (tıklama geçirme açıkken WebView fare olayı almaz).
  listen('hover', ev => document.body.classList.toggle('hover', !!ev.payload));
  window.addEventListener('resize', fit);

  // ---------------------------------------------------------------- başlangıç
  listen('caption', ev => onCaption(ev.payload));
  listen('settings', ev => { S = ev.payload; applyStyle(); render(); });
  listen('status', ev => showStatus(ev.payload));
  invoke('get_settings').then(s => { S = s; applyStyle(); });
  invoke('get_status').then(showStatus);
})();
