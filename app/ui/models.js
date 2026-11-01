(async () => {
  const T = window.__TAURI__;
  const { invoke } = T.core;
  const { listen } = T.event;
  const win = T.window.getCurrentWindow();
  const $ = id => document.getElementById(id);

  const fmt = mb => mb >= 1000 ? (mb / 1000).toFixed(1).replace('.', ',') + ' GB' : mb + ' MB';
  const bytes = b => b >= 1e9 ? (b / 1e9).toFixed(2).replace('.', ',') + ' GB' : (b / 1e6).toFixed(0) + ' MB';
  let items = [];
  let busy = false;

  function setMsg(text, cls) { $('msg').textContent = text || ''; $('msg').className = cls || ''; }

  function render(activeFile) {
    const ul = $('list');
    ul.textContent = '';
    for (const m of items) {
      const li = document.createElement('li');
      li.className = m.present ? 'done' : (m.file === activeFile ? 'now' : '');
      li.innerHTML = '<span class="mark"></span><span class="name"></span><span class="size"></span>';
      li.querySelector('.mark').textContent = m.present ? '✓' : (m.file === activeFile ? '↓' : '·');
      const name = li.querySelector('.name');
      name.textContent = m.label;
      const sm = document.createElement('small');
      sm.textContent = m.file;
      name.appendChild(sm);
      li.querySelector('.size').textContent = m.present ? 'indirildi' : fmt(m.mb);
      ul.appendChild(li);
    }
  }

  async function refresh() {
    items = await invoke('model_list');
    const todo = items.filter(m => !m.present);
    const total = todo.reduce((a, m) => a + m.mb, 0);
    render();
    if (todo.length === 0) {
      $('title').textContent = 'Tüm modeller hazır';
      $('lead').textContent = 'İndirilecek bir şey yok.';
      $('go').style.display = 'none';
      $('later').textContent = 'Kapat';
    } else {
      $('title').textContent = 'Yapay zekâ modelleri indirilecek';
      $('lead').textContent = `EchoSub’ın çalışması için ${todo.length} model dosyası eksik (toplam yaklaşık ${fmt(total)}). Hızlı bir bağlantıyla birkaç dakika sürer; indirme yarıda kesilirse kaldığı yerden devam eder.`;
      $('go').style.display = '';
      $('later').textContent = 'Şimdi değil';
    }
  }

  function setBusy(b) {
    busy = b;
    $('go').disabled = b;
    $('later').style.display = b ? 'none' : '';
    $('cancel').style.display = b ? '' : 'none';
    $('bar').style.display = b ? 'block' : 'none';
    if (!b) $('info').textContent = '';
  }

  $('go').onclick = () => {
    setMsg('');
    setBusy(true);
    $('fill').style.width = '0';
    $('info').textContent = 'Bağlanılıyor…';
    invoke('download_models');
  };
  $('cancel').onclick = () => { $('cancel').disabled = true; invoke('cancel_download'); };
  $('later').onclick = () => win.close();

  listen('dl', ev => {
    const d = ev.payload;
    if (!busy) setBusy(true);
    render(d.file);
    const pct = d.total > 0 ? Math.min(100, d.done / d.total * 100) : 0;
    $('fill').style.width = pct.toFixed(1) + '%';
    const part = d.total > 0 ? `${bytes(d.done)} / ${bytes(d.total)} (%${pct.toFixed(0)})` : bytes(d.done);
    $('info').textContent = `${d.index}/${d.count} · ${d.label} · ${part}`;
  });

  listen('dl_done', async ev => {
    setBusy(false);
    $('cancel').disabled = false;
    await refresh();
    if (ev.payload.ok) {
      setMsg('Hazır. Modeller yükleniyor, altyazı birazdan başlar.', 'ok');
      $('go').style.display = 'none';
      $('later').textContent = 'Kapat';
      setTimeout(() => win.close(), 2500);
    } else {
      const e = ev.payload.error || 'bilinmeyen hata';
      setMsg(e.includes('iptal') ? 'İndirme iptal edildi; tekrar başlatırsan kaldığı yerden devam eder.' : 'İndirilemedi: ' + e, 'err');
      $('go').textContent = 'Tekrar dene';
    }
  });

  await refresh();
})();
