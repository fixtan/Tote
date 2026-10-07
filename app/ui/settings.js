// Tote の設定画面。設定の変更は即保存。右クリック等の登録（レジストリ）は「変更を適用」で反映する。
(function () {
  'use strict';

  const { invoke } = window.__TAURI__.core;
  const $ = (id) => document.getElementById(id);

  const STATE_LABEL = {
    on: ['登録済み', 'ok'],
    off: ['未登録', 'dim'],
    stale: ['要修復（別の場所のexe／一部のみ）', 'warn'],
    na: ['使えません', 'dim'],
  };

  let state = null;          // get_state の結果
  let checks = new Map();    // 項目ID → チェックボックス
  let toastTimer = null;

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function toast(msg, opts) {
    opts = opts || {};
    $('toastMsg').textContent = msg;
    $('toastAct').hidden = true;
    $('toast').className = opts.error ? 'error' : '';
    $('toast').hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { $('toast').hidden = true; }, opts.ms || (opts.error ? 9000 : 4000));
  }

  function confirmDialog(msg, okLabel) {
    return new Promise((resolve) => {
      $('dlgMsg').textContent = msg;
      $('dlgOk').textContent = okLabel || 'OK';
      const dlg = $('dlg');
      dlg.addEventListener('close', () => resolve(dlg.returnValue === 'ok'), { once: true });
      dlg.showModal();
    });
  }

  // ------------------------------------------------------------ 登録項目

  function renderItems() {
    const box = $('items');
    checks = new Map();
    box.replaceChildren();
    for (const it of state.items) {
      const row = el('label', 'item');
      const cb = el('input');
      cb.type = 'checkbox';
      cb.checked = it.state === 'on' || it.state === 'stale';
      cb.disabled = it.state === 'na';
      cb.dataset.id = it.id;
      checks.set(it.id, cb);
      const body = el('div', 'item-body');
      body.appendChild(el('div', 'item-title', it.label));
      body.appendChild(el('div', 'item-hint dim', it.hint));
      const [text, cls] = STATE_LABEL[it.state] || ['', 'dim'];
      const badge = el('span', 'state ' + cls, text);
      row.append(cb, body, badge);
      box.appendChild(row);
    }
  }

  function setItems(items) {
    state.items = items;
    renderItems();
  }

  function wantList() {
    return state.items
      .filter((it) => it.state !== 'na')
      .map((it) => [it.id, checks.get(it.id).checked]);
  }

  async function applyItems() {
    try {
      setItems(await invoke('apply_items', { wants: wantList() }));
      toast('登録を更新しました');
    } catch (err) {
      toast(String(err), { error: true });
      await reload();
    }
  }

  async function repair() {
    try {
      const n = await invoke('repair_items');
      await reload();
      toast(n ? `${n} 項目を今の場所で登録し直しました` : '登録済みの項目はありません');
    } catch (err) { toast(String(err), { error: true }); }
  }

  async function uninstall() {
    const ok = await confirmDialog('Tote の右クリックメニュー・「送る」・関連付けの候補をすべて解除します。\n（設定は消えません）', '解除する');
    if (!ok) return;
    try {
      setItems(await invoke('uninstall_everything'));
      toast('すべて解除しました');
    } catch (err) { toast(String(err), { error: true }); }
  }

  // ------------------------------------------------------------ 設定

  function fillSelect(sel, options, value) {
    sel.replaceChildren(...options.map(([v, label]) => {
      const o = el('option', '', label);
      o.value = v;
      return o;
    }));
    sel.value = value;
  }

  function renderConfig() {
    const c = state.config;
    fillSelect($('fmt'), state.formats.map((f) => [f.id, f.label]), c.compressFormat);
    renderLevels();
    $('reveal').value = c.reveal;
    $('confirmRisky').checked = c.confirmRisky;
    $('extractDest').value = c.extractDest;
    document.querySelectorAll('input[name="append"]').forEach((r) => { r.checked = r.value === c.appendMode; });
  }

  function renderLevels() {
    const f = state.formats.find((x) => x.id === $('fmt').value) || state.formats[0];
    const cur = state.config.compressLevel;
    $('level').closest('.field').hidden = f.presets.length === 0;
    if (!f.presets.length) { $('level').replaceChildren(); return; }
    fillSelect($('level'), f.presets.map((p) => [p.id, p.label]), f.presets.some((p) => p.id === cur) ? cur : (f.presets.find((p) => p.id === 'normal') || f.presets[0]).id);
  }

  function readConfig() {
    const r = document.querySelector('input[name="append"]:checked');
    return {
      compressFormat: $('fmt').value,
      compressLevel: $('level').value || state.config.compressLevel,
      compressSolid: state.config.compressSolid !== false,
      confirmRisky: $('confirmRisky').checked,
      reveal: $('reveal').value,
      extractDest: $('extractDest').value,
      appendMode: r ? r.value : state.config.appendMode,
    };
  }

  async function saveConfig() {
    try {
      state.config = await invoke('save_config', { config: readConfig() });
      toast('設定を保存しました', { ms: 1800 });
    } catch (err) { toast(String(err), { error: true }); }
  }

  async function resetConfig() {
    const ok = await confirmDialog('設定を初期値に戻します。（右クリックなどの登録はそのままです）', '初期化する');
    if (!ok) return;
    try {
      state.config = await invoke('reset_config');
      renderConfig();
      toast('設定を初期化しました');
    } catch (err) { toast(String(err), { error: true }); }
  }

  // ------------------------------------------------------------ 起動

  function renderStatic() {
    $('ver').textContent = 'v' + state.version;
    $('paths').textContent = `実行ファイル: ${state.exe}\n設定ファイル: ${state.configPath}`;
    $('regNote').textContent = state.supported
      ? 'チェックを入れた項目を登録します（管理者権限は不要）。Windows 11 では右クリックの項目は「その他のオプションを確認」の中に出ます。' +
        'tote.exe を別の場所へ移動したら「壊れた登録を修復」を押してください。'
      : 'この環境（Windows 以外）では右クリック登録は使えません。';
    for (const id of ['btnApply', 'btnRepair', 'btnDefaultApps', 'btnUninstall']) $(id).disabled = !state.supported;
  }

  async function reload() {
    state = await invoke('get_state');
    renderStatic();
    renderItems();
    renderConfig();
  }

  $('btnApply').addEventListener('click', applyItems);
  $('btnRepair').addEventListener('click', repair);
  $('btnUninstall').addEventListener('click', uninstall);
  $('btnReset').addEventListener('click', resetConfig);
  $('btnDefaultApps').addEventListener('click', async () => {
    try { await invoke('open_default_apps'); } catch (err) { toast(String(err), { error: true }); }
  });
  $('fmt').addEventListener('change', () => { renderLevels(); saveConfig(); });
  for (const id of ['level', 'reveal', 'confirmRisky', 'extractDest']) $(id).addEventListener('change', saveConfig);
  document.querySelectorAll('input[name="append"]').forEach((r) => r.addEventListener('change', saveConfig));
  window.addEventListener('contextmenu', (ev) => ev.preventDefault());

  reload().catch((err) => {
    document.body.replaceChildren(el('div', 'warn', `設定を読み込めませんでした: ${String(err)}`));
  });
})();
