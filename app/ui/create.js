// 「書庫を作成」ダイアログ。パスワードは画面とバックエンドの間でしか持たない（保存しない）。
(function () {
  'use strict';

  const { invoke } = window.__TAURI__.core;
  const $ = (id) => document.getElementById(id);

  let init = null;      // create_init の結果
  let dir = '';
  let busy = false;
  let output = '';      // 作成した書庫のパス

  const fmt = () => init.formats.find((f) => f.id === $('fmt').value) || init.formats[0];

  function fillSelect(sel, options, value) {
    sel.replaceChildren(...options.map(([v, label]) => {
      const o = document.createElement('option');
      o.value = v;
      o.textContent = label;
      return o;
    }));
    sel.value = value;
  }

  function showError(msg) {
    $('err').textContent = msg || '';
    $('err').hidden = !msg;
  }

  function confirmDialog(msg, okLabel) {
    return new Promise((resolve) => {
      $('dlgMsg').textContent = msg;
      $('dlgOk').textContent = okLabel || 'OK';
      const dlg = $('dlg');
      dlg.addEventListener('close', () => resolve(dlg.returnValue === 'ok'), { once: true });
      dlg.returnValue = 'cancel';
      dlg.showModal();
    });
  }

  // 形式に合わせて、選べない項目を隠す
  function applyFormat() {
    const f = fmt();
    $('ext').textContent = '.' + f.extension;
    const prevLevel = $('level').value;
    $('rowLevel').hidden = f.presets.length === 0;
    if (f.presets.length) {
      fillSelect($('level'), f.presets.map((p) => [p.id, p.label]), f.presets.some((p) => p.id === prevLevel) ? prevLevel : (f.presets.find((p) => p.id === 'normal') || f.presets[0]).id);
    }
    $('rowSolid').hidden = !f.supportsSolid;
    $('pwSec').hidden = !f.supportsPassword;
    $('rowNames').hidden = !f.supportsNameEncryption;
    $('pwNote').textContent = f.id === 'zip'
      ? 'ZIP のパスワードは AES-256 です。Windows のエクスプローラーでは開けません（7-Zip・WinRAR・Tote なら開けます）。'
      : f.id === '7z' ? '7z のパスワードは AES-256 です。「ファイル名も暗号化」を付けない場合、中身の一覧だけは見えます。' : '';
    updatePwState();
  }

  function updatePwState() {
    const has = $('pw').value.length > 0;
    $('encNames').disabled = !has;
    if (!has) $('encNames').checked = false;
  }

  function validate() {
    const f = fmt();
    const stem = $('stem').value.trim();
    if (!dir) return 'まず「保存先」を選んでください';
    if (!stem) return '名前を入力してください';
    if (/[\\/:*?"<>|]/.test(stem)) return '名前に使えない文字があります（\\ / : * ? " < > |）';
    if (/[. ]$/.test(stem)) return '名前の末尾に「.」や空白は付けられません';
    if (f.supportsPassword && $('pw').value !== $('pw2').value) return 'パスワードが一致しません';
    return '';
  }

  function setBusy(on) {
    busy = on;
    for (const id of ['fmt', 'level', 'solid', 'pw', 'pw2', 'showPw', 'encNames', 'btnDir', 'stem', 'btnCancel', 'btnCreate']) $(id).disabled = on;
    $('btnCreate').textContent = on ? '作成中…' : '作成';
    if (!on) updatePwState();
  }

  async function run(overwrite) {
    const f = fmt();
    const pw = f.supportsPassword ? $('pw').value : '';
    const req = {
      format: f.id,
      level: f.presets.length ? $('level').value : '',
      password: pw || null,
      solid: f.supportsSolid ? $('solid').checked : true,
      encryptNames: !!(pw && f.supportsNameEncryption && $('encNames').checked),
      dir,
      stem: $('stem').value.trim(),
      overwrite: !!overwrite,
    };
    setBusy(true);
    try {
      const r = await invoke('create_run', { req });
      if (r.status === 'exists') {
        setBusy(false);
        const ok = await confirmDialog(`同じ名前のファイルがあります。\n${r.output}\n\n上書きしますか？`, '上書きする');
        if (ok) await run(true);
        return;
      }
      output = r.output;
      $('form').hidden = true;
      $('result').hidden = false;
      $('resText').textContent =
        `${r.output}\n${r.files.toLocaleString('ja-JP')} 個のファイル、${r.dirs.toLocaleString('ja-JP')} 個のフォルダを入れました（${fmtBytes(r.size)}）` +
        (r.skipped ? `\n${r.skipped} 個の項目（リンクなど）はスキップしました` : '') +
        (pw ? '\nパスワードを付けました。忘れると開けなくなります' : '');
      $('btnReveal').focus();
    } catch (err) {
      setBusy(false);
      showError(String(err));
    }
  }

  function fmtBytes(n) {
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i === 0 ? n : n.toFixed(1)) + ' ' + u[i];
  }

  async function create() {
    if (busy) return;
    showError('');
    const e = validate();
    if (e) { showError(e); return; }
    await run(false);
  }

  async function pickDir() {
    try {
      const d = await invoke('create_pick_dir', { current: dir });
      if (d) { dir = d; $('dir').textContent = dir; $('dir').title = dir; showError(''); }
    } catch (err) { showError(String(err)); }
  }

  const close = () => invoke('create_close');

  async function boot() {
    try {
      init = await invoke('create_init');
    } catch (err) {
      showError(String(err));
      return;
    }
    const names = init.names.join('、');
    $('src').textContent = `${init.count.toLocaleString('ja-JP')} 個の項目: ${names}${init.count > init.names.length ? ' ほか' : ''}`;
    $('src').title = $('src').textContent;
    fillSelect($('fmt'), init.formats.map((f) => [f.id, f.label]), init.format);
    fillSelect($('level'), [], '');
    $('solid').checked = init.solid;
    // 形式を決めてから、保存済みのレベルを入れる
    const f = fmt();
    if (f.presets.length) fillSelect($('level'), f.presets.map((p) => [p.id, p.label]), f.presets.some((p) => p.id === init.level) ? init.level : f.presets[0].id);
    dir = init.dir;
    $('dir').textContent = dir || '（未選択）';
    $('dir').title = dir;
    $('stem').value = init.stem;
    applyFormat();
    $('stem').focus();
    $('stem').select();
    if (!dir) showError('保存先を決められませんでした。「変更…」から選んでください');
  }

  $('fmt').addEventListener('change', applyFormat);
  $('pw').addEventListener('input', updatePwState);
  $('showPw').addEventListener('change', () => {
    const t = $('showPw').checked ? 'text' : 'password';
    $('pw').type = t;
    $('pw2').type = t;
  });
  $('btnDir').addEventListener('click', pickDir);
  $('btnCreate').addEventListener('click', create);
  $('btnCancel').addEventListener('click', close);
  $('btnClose').addEventListener('click', close);
  $('btnReveal').addEventListener('click', async () => {
    try { await invoke('create_reveal', { path: output }); } catch (err) { $('resText').textContent = String(err); }
  });
  window.addEventListener('keydown', (ev) => {
    if ($('dlg').open) return;
    if (ev.key === 'Escape' && !busy) { ev.preventDefault(); close(); }
    else if (ev.key === 'Enter' && !$('form').hidden && ev.target.tagName !== 'BUTTON' && ev.target.tagName !== 'SELECT') { ev.preventDefault(); create(); }
  });
  window.addEventListener('contextmenu', (ev) => ev.preventDefault());

  boot();
})();
