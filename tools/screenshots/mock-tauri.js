// Gerçek arayüz dosyalarını (app/ui) Tauri olmadan çalıştırıp ekran görüntüsü almak için sahte API.
// Durum URL parametreleriyle verilir: ?lang=tr&hover=1&orig=...&text=...&size=...&second=...
(() => {
  const q = new URLSearchParams(location.search);
  const handlers = {};
  const settings = Object.assign({
    target_lang: 'tr', source_lang: 'auto', quality: 'balanced',
    font_family: 'Segoe UI, Arial, sans-serif', font_scale: 1, auto_fit: true, bold: true,
    text_color: '#ffffff', outline_color: '#000000', outline_width: 3, bg_color: '#000000',
    bg_opacity: 0, lines: 2, show_original: q.get('orig') === '1', hide_after_secs: 3600,
    use_vad: true, use_separation: true, click_through: true, max_caption_age_secs: 15, hotkey: 'Ctrl+Shift+F11',
  }, q.get('settings') ? JSON.parse(q.get('settings')) : {});
  const langs = [['en','English'],['ja','Japanese'],['ru','Russian'],['de','German'],['es','Spanish'],
                 ['fr','French'],['ko','Korean'],['tr','Turkish'],['ar','Arabic'],['zh','Chinese']];
  window.__TAURI__ = {
    core: { invoke: async cmd => ({
      get_settings: settings, get_status: { stage: 'ready', msg: '' }, languages: langs,
    })[cmd] ?? null },
    event: { listen: (name, cb) => { handlers[name] = cb; return Promise.resolve(() => {}); } },
    window: { getCurrentWindow: () => ({ startDragging() {}, startResizeDragging() {} }) },
  };
  window.addEventListener('load', () => setTimeout(() => {
    if (q.get('hover') === '1') handlers.hover && handlers.hover({ payload: true });
    const lines = JSON.parse(q.get('lines') || '[]');
    lines.forEach((l, i) => handlers.caption && handlers.caption({ payload: {
      seg_id: i + 1, final: true, clear: false, lang: l.lang || 'ja',
      original: l.orig || '', translated: l.text, latency_ms: l.late || 220 } }));
  }, 80));
})();
