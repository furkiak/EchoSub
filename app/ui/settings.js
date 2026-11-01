(async () => {
  const T = window.__TAURI__;
  const { invoke } = T.core;
  const { listen } = T.event;
  const $ = id => document.getElementById(id);

  const langs = await invoke('languages');
  const opt = (v, t) => { const o = document.createElement('option'); o.value = v; o.textContent = t; return o; };
  for (const [c, n] of langs) $('target_lang').appendChild(opt(c, `${n} (${c})`));
  $('source_lang').appendChild(opt('auto', 'Otomatik algıla'));
  for (const [c, n] of langs) $('source_lang').appendChild(opt(c, `${n} (${c})`));

  const FIELDS = {
    target_lang: 'v', source_lang: 'v', quality: 'v', font_family: 'v', text_color: 'v',
    outline_color: 'v', bg_color: 'v',
    show_original: 'c', use_vad: 'c', use_separation: 'c', click_through: 'c', auto_fit: 'c', bold: 'c',
    font_scale: 'n', outline_width: 'n', bg_opacity: 'n', lines: 'n', hide_after_secs: 'n',
  };

  let S = await invoke('get_settings');

  function fill() {
    for (const [k, t] of Object.entries(FIELDS)) {
      const el = $(k);
      if (t === 'c') el.checked = !!S[k]; else el.value = S[k];
    }
    for (const k of ['font_scale', 'outline_width', 'bg_opacity']) {
      $(k + '_v').textContent = k === 'bg_opacity' ? Math.round(S[k] * 100) + '%' : (+S[k]).toFixed(2).replace(/\.?0+$/, '');
    }
    $('scale_label').textContent = S.auto_fit ? 'Yazı boyutu (pencereye göre)' : 'Yazı boyutu (çarpan)';
    $('font_scale').max = S.auto_fit ? '1' : '3';
    preview();
  }

  function preview() {
    const p = $('pv');
    p.style.fontFamily = S.font_family;
    p.style.fontWeight = S.bold ? '700' : '400';
    p.style.color = S.text_color;
    p.style.fontSize = (S.auto_fit ? 26 * Math.min(1, S.font_scale) + 4 : 22 * S.font_scale) + 'px';
    p.style.webkitTextStroke = S.outline_width > 0 ? `${S.outline_width * 2}px ${S.outline_color}` : '0';
    p.style.paintOrder = 'stroke fill';
    const hex = S.bg_color.replace('#', '');
    const [r, g, b] = [0, 2, 4].map(i => parseInt(hex.slice(i, i + 2), 16));
    p.style.background = `rgba(${r},${g},${b},${S.bg_opacity})`;
    p.style.padding = '4px 10px';
    p.style.borderRadius = '8px';
  }

  let timer = null;
  function push() {
    clearTimeout(timer);
    timer = setTimeout(() => invoke('set_settings', { new: S }), 150);
  }

  for (const [k, t] of Object.entries(FIELDS)) {
    const el = $(k);
    el.addEventListener(t === 'c' ? 'change' : 'input', () => {
      const n = parseFloat(el.value);
      if (t === 'n' && !Number.isFinite(n)) return; // sayı alanı yazılırken boş kalabilir
      S[k] = t === 'c' ? el.checked : t === 'n' ? n : el.value;
      if (k === 'lines') S[k] = Math.max(1, Math.min(6, Math.round(S[k] || 2)));
      if (k === 'auto_fit') S.font_scale = S.auto_fit ? Math.min(1, S.font_scale) : S.font_scale;
      fill();
      push();
    });
  }

  $('open_models').addEventListener('click', () => invoke('open_models'));

  const show = s => { $('status').textContent = s.msg ? `Durum: ${s.msg}` : ''; };
  listen('status', ev => show(ev.payload));
  invoke('get_status').then(show);
  fill();
})();
