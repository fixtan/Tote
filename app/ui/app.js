// Tote 書庫ビューア。ファイル名は信用できない入力なので、画面への出力は必ず textContent で行う。
(function () {
  'use strict';

  const { invoke } = window.__TAURI__.core;
  const T = window.ToteTree;
  const $ = (id) => document.getElementById(id);

  const els = {
    rows: $('rows'), crumbs: $('crumbs'), warn: $('warn'), empty: $('empty'), wrap: $('wrap'),
    stInfo: $('stInfo'), stSel: $('stSel'), btnUp: $('btnUp'),
    btnAll: $('btnExtractAll'), btnSel: $('btnExtractSel'), btnSettings: $('btnSettings'),
    toast: $('toast'), toastMsg: $('toastMsg'), toastAct: $('toastAct'),
    drop: $('drop'), dropBox: $('dropBox'),
    dlg: $('dlg'), dlgMsg: $('dlgMsg'), dlgOk: $('dlgOk'),
  };

  let archive = null;      // { path, name, info }
  let root = null;         // ツリーのルート
  let stats = null;
  let cwd = '';            // 今いるフォルダ（ZIP内パス。ルートは ''）
  let sortKey = 'name';
  let sortDir = 1;
  let selected = new Set();
  let anchor = null;       // Shift選択の起点
  let shown = [];          // 画面に出ているノード（並び順）
  let busy = false;
  let cfg = { confirmRisky: true };

  // ------------------------------------------------------------ ツリー操作

  function nodeAt(path) {
    let n = root;
    for (const c of path.split('/').filter(Boolean)) {
      n = n.children.get(c);
      if (!n) return root;
    }
    return n;
  }

  function parentOf(path) {
    const i = path.lastIndexOf('/');
    return i < 0 ? '' : path.slice(0, i);
  }

  function cd(path) {
    cwd = path;
    selected = new Set();
    anchor = null;
    render();
    els.wrap.scrollTop = 0;
  }

  // ------------------------------------------------------------ 描画

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function renderCrumbs() {
    els.crumbs.replaceChildren();
    const parts = cwd.split('/').filter(Boolean);
    const add = (label, path, current) => {
      const b = el('button', current ? 'current' : '', label);
      b.title = label;
      b.addEventListener('click', () => cd(path));
      els.crumbs.appendChild(b);
    };
    add(archive.name, '', parts.length === 0);
    parts.forEach((p, i) => {
      els.crumbs.appendChild(el('span', 'sep', '›'));
      add(p, parts.slice(0, i + 1).join('/'), i === parts.length - 1);
    });
  }

  function rowFor(n) {
    const e = n.entry;
    const tr = document.createElement('tr');
    tr.dataset.path = n.path;
    if (selected.has(n.path)) tr.classList.add('sel');
    if (e && e.risky) tr.classList.add('risky');
    if (e && !e.safe) tr.classList.add('unsafe');

    const name = el('td', 'name');
    name.appendChild(el('span', 'ico', n.isDir ? '📁' : '📄'));
    name.appendChild(el('span', 'label', n.name));
    if (e && e.risky) { const b = el('span', 'badge', '⚠'); b.title = '実行形式・スクリプトです。開く前に内容を確認してください'; name.appendChild(b); }
    if (e && e.encrypted && !n.isDir) { const b = el('span', 'badge', '🔒'); b.title = 'パスワード付き（展開は未対応です）'; name.appendChild(b); }
    if (e && !e.safe) { const b = el('span', 'badge', '✖'); b.title = '不正なパスのため展開されません'; name.appendChild(b); }
    name.title = n.name;
    tr.appendChild(name);

    const num = (v) => el('td', 'num', v === undefined || n.isDir ? '' : v.toLocaleString('ja-JP'));
    tr.appendChild(num(e ? e.size : undefined));
    // 固体圧縮などで個別の格納サイズが無い形式は 0 で来る。0 と区別して空欄にする
    tr.appendChild(num(e && !(e.packed === 0 && e.size > 0) ? e.packed : undefined));
    tr.appendChild(el('td', 'dim', T.typeLabel(n)));
    tr.appendChild(el('td', 'dim', e && e.modified ? e.modified : ''));
    return tr;
  }

  function render() {
    renderCrumbs();
    const dir = nodeAt(cwd);
    shown = [...dir.children.values()].sort(T.compareNodes(sortKey, sortDir));

    const frag = document.createDocumentFragment();
    if (cwd !== '') {
      const up = document.createElement('tr');
      up.dataset.up = '1';
      const name = el('td', 'name');
      name.appendChild(el('span', 'ico', '📁'));
      name.appendChild(el('span', 'label', '..'));
      up.appendChild(name);
      for (let i = 0; i < 4; i++) up.appendChild(el('td'));
      frag.appendChild(up);
    }
    for (const n of shown) frag.appendChild(rowFor(n));
    els.rows.replaceChildren(frag);

    els.empty.hidden = shown.length > 0 || cwd !== '';
    els.btnUp.disabled = cwd === '';
    document.querySelectorAll('thead th').forEach((th) => {
      const on = th.dataset.key === sortKey;
      th.classList.toggle('sorted', on);
      th.classList.toggle('desc', on && sortDir < 0);
    });
    updateStatus();
  }

  function updateSelectionView() {
    els.rows.querySelectorAll('tr[data-path]').forEach((tr) => {
      tr.classList.toggle('sel', selected.has(tr.dataset.path));
    });
    updateStatus();
  }

  function updateStatus() {
    els.stInfo.textContent =
      `${stats.files.toLocaleString('ja-JP')} ファイル / ${stats.dirs.toLocaleString('ja-JP')} フォルダ` +
      ` ・ 展開後 ${T.fmtBytes(stats.total)} ・ 圧縮後 ${T.fmtBytes(stats.packed || archive.info.totalPacked || 0)}` +
      (archive.info.format ? ` ・ ${archive.info.format}` : '');
    if (selected.size === 0) { els.stSel.textContent = ''; return; }
    let size = 0;
    for (const p of selected) {
      const n = nodeAt(p);
      if (n.entry && !n.isDir) size += n.entry.size;
    }
    els.stSel.textContent = `${selected.size} 項目を選択` + (size ? `（${T.fmtBytes(size)}）` : '');
  }

  function renderWarnings() {
    const msgs = [];
    if (stats.risky) msgs.push(`⚠ 実行形式・スクリプトが ${stats.risky} 個含まれています。開く前に内容を確認してください。`);
    if (stats.unsafe) msgs.push(`✖ 不正なパス（..を含む）の項目が ${stats.unsafe} 個あります。これらは展開されません。`);
    if (stats.encrypted) msgs.push(`🔒 パスワード付きの項目が ${stats.encrypted} 個あります（展開は未対応です）。`);
    if (archive.info.comment) msgs.push(`コメント: ${archive.info.comment}`);
    els.warn.replaceChildren(...msgs.map((m) => el('div', '', m)));
    els.warn.hidden = msgs.length === 0;
  }

  // ------------------------------------------------------------ 通知とダイアログ

  let toastTimer = null;
  function toast(msg, opts = {}) {
    clearTimeout(toastTimer);
    els.toastMsg.textContent = msg;
    els.toast.classList.toggle('error', !!opts.error);
    if (opts.action) {
      els.toastAct.textContent = opts.action.label;
      els.toastAct.onclick = () => { opts.action.fn(); hideToast(); };
      els.toastAct.hidden = false;
    } else {
      els.toastAct.hidden = true;
      els.toastAct.onclick = null;
    }
    els.toast.hidden = false;
    if (!opts.sticky) toastTimer = setTimeout(hideToast, opts.ms || (opts.error ? 8000 : 5000));
  }
  function hideToast() { clearTimeout(toastTimer); els.toast.hidden = true; }

  function confirmDialog(message, okLabel) {
    return new Promise((resolve) => {
      els.dlgMsg.textContent = message;
      els.dlgOk.textContent = okLabel;
      els.dlg.addEventListener('close', () => resolve(els.dlg.returnValue === 'ok'), { once: true });
      els.dlg.returnValue = 'cancel';
      els.dlg.showModal();
    });
  }

  // ------------------------------------------------------------ 操作

  async function openNode(n) {
    if (n.isDir) { cd(n.path); return; }
    const e = n.entry;
    if (e && e.encrypted) { toast('パスワード付きのファイルは未対応です', { error: true }); return; }
    if (e && !e.safe) { toast('不正なパスのため開けません', { error: true }); return; }
    if (e && e.risky && cfg.confirmRisky) {
      const ok = await confirmDialog(
        `「${n.name}」は実行形式またはスクリプトです。\n開くとプログラムが実行される可能性があります。開きますか？`, '開く');
      if (!ok) return;
    }
    try {
      toast('開いています…', { sticky: true });
      await invoke('open_entry', { path: n.path });
      hideToast();
    } catch (err) { toast(String(err), { error: true }); }
  }

  function reportDone(done) {
    const skipped = done.skipped.length;
    let msg = `${done.files.toLocaleString('ja-JP')} 個のファイルを展開しました`;
    if (skipped) msg += `（${skipped} 個をスキップ: ${done.skipped[0].path} — ${done.skipped[0].reason}${skipped > 1 ? ' ほか' : ''}）`;
    toast(msg, { ms: 12000, error: skipped > 0 && done.files === 0, action: { label: '展開先を開く', fn: () => invoke('open_folder', { path: done.dest }) } });
  }

  async function withBusy(fn) {
    if (busy) return;
    busy = true;
    els.btnAll.disabled = els.btnSel.disabled = true;
    try { await fn(); }
    catch (err) { toast(String(err), { error: true }); }
    finally { busy = false; els.btnAll.disabled = els.btnSel.disabled = false; }
  }

  function extractAll() {
    return withBusy(async () => {
      if (cfg.extractDest !== 'ask') toast('展開中…', { sticky: true });
      const done = await invoke('extract_all');
      if (!done) { hideToast(); return; } // 展開先の選択がキャンセルされた
      reportDone(done);
    });
  }

  function extractSelected() {
    if (selected.size === 0) { toast('展開する項目を選択してください'); return; }
    return withBusy(async () => {
      const done = await invoke('extract_selected', { paths: [...selected] });
      if (!done) return; // 展開先の選択がキャンセルされた
      reportDone(done);
    });
  }

  // ------------------------------------------------------------ 選択とドラッグ展開

  let press = null; // { x, y, id, dragged, collapseTo, released }

  function selectRange(toPath) {
    const order = shown.map((n) => n.path);
    const a = order.indexOf(anchor), b = order.indexOf(toPath);
    if (a < 0 || b < 0) { selected = new Set([toPath]); return; }
    const [s, e] = a < b ? [a, b] : [b, a];
    selected = new Set(order.slice(s, e + 1));
  }

  els.rows.addEventListener('pointerdown', (ev) => {
    const tr = ev.target.closest('tr');
    if (!tr || ev.button !== 0) return;
    if (tr.dataset.up) { selected = new Set(); anchor = null; updateSelectionView(); return; }
    const path = tr.dataset.path;
    let collapseTo = null;

    if (ev.shiftKey && anchor !== null) {
      selectRange(path);
    } else if (ev.ctrlKey || ev.metaKey) {
      if (selected.has(path)) selected.delete(path); else selected.add(path);
      anchor = path;
    } else {
      if (selected.has(path) && selected.size > 1) collapseTo = path; // ドラッグでなければ、離したときに1つに絞る
      else selected = new Set([path]);
      anchor = path;
    }
    updateSelectionView();

    if (!ev.shiftKey && !ev.ctrlKey && !ev.metaKey && selected.has(path)) {
      press = { x: ev.clientX, y: ev.clientY, id: ev.pointerId, dragged: false, collapseTo, released: false };
      try { tr.setPointerCapture(ev.pointerId); } catch (_) { /* 取れなくても動作に支障なし */ }
    }
  });

  window.addEventListener('pointermove', (ev) => {
    if (!press || press.dragged || press.id !== ev.pointerId) return;
    if (Math.hypot(ev.clientX - press.x, ev.clientY - press.y) < 6) return;
    press.dragged = true;
    beginDrag();
  });

  window.addEventListener('pointerup', (ev) => {
    if (!press || press.id !== ev.pointerId) return;
    press.released = true;
    if (!press.dragged && press.collapseTo) {
      selected = new Set([press.collapseTo]);
      updateSelectionView();
    }
    if (!press.dragged) press = null;
  });

  window.addEventListener('pointercancel', () => { if (press) press.released = true; });

  async function beginDrag() {
    const state = press;
    const paths = [...selected];
    const timer = setTimeout(() => toast('展開中…', { sticky: true }), 300);
    try {
      await invoke('prepare_drag', { paths });
    } catch (err) {
      clearTimeout(timer);
      press = null;
      toast(String(err), { error: true });
      return;
    }
    clearTimeout(timer);
    hideToast();
    if (state.released) {
      // 展開に時間がかかって、ボタンはもう離されている。展開済みなので次のドラッグは即座に始まる。
      press = null;
      toast('展開が終わりました。もう一度ドラッグしてください');
      return;
    }
    press = null;
    try { await invoke('start_drag', { paths }); }
    catch (err) { toast(String(err), { error: true }); }
  }

  // ------------------------------------------------------------ ドロップで追加

  const canAdd = () => !!archive && archive.info.format === 'ZIP';
  const ownDrag = (paths) => paths.length > 0 && paths.every((p) => /tote-(drag|open)/.test(p));

  function showDrop(on, paths = []) {
    if (!on || !archive || ownDrag(paths)) { els.drop.hidden = true; return; }
    const ok = canAdd();
    els.drop.classList.toggle('deny', !ok);
    els.dropBox.textContent = ok
      ? `ここにドロップして追加\n（追加先: ${cwd === '' ? archive.name : cwd}）`
      : `${archive.info.format || 'この形式'} の書庫には追加できません\n（追加できるのは ZIP のみです）`;
    els.drop.hidden = false;
  }

  async function reload() {
    archive = await invoke('load_archive');
    root = T.buildTree(archive.info.entries);
    stats = T.stats(archive.info.entries, root);
    if (cwd !== '' && nodeAt(cwd) === root) cwd = '';
    selected = new Set();
    anchor = null;
    renderWarnings();
    render();
  }

  function addDropped(paths) {
    if (!paths.length || ownDrag(paths)) return;
    if (!canAdd()) { toast(`${archive ? archive.info.format : 'この形式'} の書庫には追加できません（ZIPのみ）`, { error: true }); return; }
    return withBusy(async () => {
      toast('追加中…', { sticky: true });
      const r = await invoke('add_files', { paths, dest: cwd });
      if (!r) { hideToast(); return; }
      await reload();
      let msg = `${r.files.toLocaleString('ja-JP')} 個のファイルを追加しました`;
      if (r.replaced) msg += `（同名の ${r.replaced} 個を置き換え）`;
      if (r.skipped) msg += `（${r.skipped} 個のリンク等はスキップ）`;
      if (r.fellBack) msg += '。置き換えがあったため安全な方式で書き込みました';
      toast(msg, { ms: 10000 });
    });
  }

  const ev = window.__TAURI__.event;
  if (ev) {
    ev.listen('tauri://drag-enter', (e) => showDrop(true, (e.payload && e.payload.paths) || []));
    ev.listen('tauri://drag-leave', () => showDrop(false));
    ev.listen('tauri://drag-drop', (e) => { showDrop(false); addDropped((e.payload && e.payload.paths) || []); });
  }

  // ------------------------------------------------------------ イベント

  els.rows.addEventListener('dblclick', (ev) => {
    const tr = ev.target.closest('tr');
    if (!tr) return;
    if (tr.dataset.up) { cd(parentOf(cwd)); return; }
    openNode(nodeAt(tr.dataset.path));
  });

  document.querySelector('thead').addEventListener('click', (ev) => {
    const th = ev.target.closest('th');
    if (!th) return;
    const key = th.dataset.key;
    if (key === sortKey) sortDir = -sortDir; else { sortKey = key; sortDir = 1; }
    render();
  });

  els.btnUp.addEventListener('click', () => cd(parentOf(cwd)));
  els.btnAll.addEventListener('click', extractAll);
  els.btnSel.addEventListener('click', extractSelected);
  els.btnSettings.addEventListener('click', async () => {
    try { await invoke('open_settings'); } catch (err) { toast(String(err), { error: true }); }
  });

  els.wrap.addEventListener('pointerdown', (ev) => {
    if (ev.target === els.wrap || ev.target === els.empty) { selected = new Set(); updateSelectionView(); }
  });

  window.addEventListener('keydown', (ev) => {
    if (els.dlg.open) return;
    if (ev.key === 'Backspace') { ev.preventDefault(); if (cwd !== '') cd(parentOf(cwd)); }
    else if (ev.key === 'Enter') {
      if (selected.size === 1) { ev.preventDefault(); openNode(nodeAt([...selected][0])); }
    } else if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'a') {
      ev.preventDefault();
      selected = new Set(shown.map((n) => n.path));
      updateSelectionView();
    } else if (ev.key === 'Escape') {
      selected = new Set();
      updateSelectionView();
    }
  });

  // 画面全体の既定の右クリックメニュー（更新・ソース表示など）は出さない
  window.addEventListener('contextmenu', (ev) => ev.preventDefault());

  // ------------------------------------------------------------ 起動

  async function init() {
    try { cfg = Object.assign(cfg, await invoke('get_config')); } catch (_) { /* 設定が読めなくても既定値で動く */ }
    try {
      archive = await invoke('load_archive');
    } catch (err) {
      document.body.replaceChildren(el('div', 'warn', `開けませんでした: ${String(err)}`));
      return;
    }
    root = T.buildTree(archive.info.entries);
    stats = T.stats(archive.info.entries, root);
    renderWarnings();
    render();
  }

  init();
})();
