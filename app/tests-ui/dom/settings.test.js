// 設定画面（app/ui/settings.html）の動作テスト。jsdom が必要: `cd app/tests-ui && npm install && npm run test:dom`
const { JSDOM } = require('jsdom');
const assert = require('node:assert');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const baseItems = () => [
  { id: 'compress', label: '右クリックメニュー「ToteでZIPに圧縮」', hint: 'h1', state: 'on' },
  { id: 'sendto', label: '「送る」メニュー', hint: 'h2', state: 'off' },
  { id: 'open:zip', label: 'ZIP を開く', hint: 'h3', state: 'stale' },
  { id: 'open:7z', label: '7z を開く', hint: 'h4', state: 'off' },
];

async function boot({ supported = true, failApply = false } = {}) {
  const calls = [];
  const st = {
    config: { compressFormat: 'zip', compressLevel: 'normal', confirmRisky: true, reveal: 'slow', extractDest: 'besideArchive', appendMode: 'safe' },
    items: baseItems(),
    formats: [{ id: 'zip', label: 'ZIP', presets: [{ id: 'store', label: '圧縮しない' }, { id: 'normal', label: '標準' }, { id: 'best', label: '最高' }] }],
    exe: 'C:\\Tote\\tote.exe', configPath: 'C:\\x\\config.json', version: '0.3.0', supported,
  };
  const invoke = async (cmd, args) => {
    calls.push([cmd, JSON.parse(JSON.stringify(args || {}))]);
    switch (cmd) {
      case 'get_state': return JSON.parse(JSON.stringify(st));
      case 'save_config': st.config = args.config; return st.config;
      case 'reset_config': st.config = { compressFormat: 'zip', compressLevel: 'normal', confirmRisky: true, reveal: 'slow', extractDest: 'besideArchive', appendMode: 'safe' }; return st.config;
      case 'apply_items':
        if (failApply) throw 'レジストリ書き込みに失敗: アクセスが拒否されました';
        for (const [id, on] of args.wants) { const it = st.items.find((x) => x.id === id); it.state = on ? 'on' : 'off'; }
        return JSON.parse(JSON.stringify(st.items));
      case 'repair_items': for (const it of st.items) if (it.state === 'stale') it.state = 'on'; return 1;
      case 'uninstall_everything': st.items.forEach((i) => { i.state = 'off'; }); return JSON.parse(JSON.stringify(st.items));
      default: return null;
    }
  };
  const dom = await JSDOM.fromFile(require('node:path').join(__dirname, '../../ui/settings.html'), {
    runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
    beforeParse(w) { w.__TAURI__ = { core: { invoke } }; },
  });
  await new Promise((r) => dom.window.addEventListener('load', r));
  await sleep(50);
  const w = dom.window, d = w.document;
  const dlg = d.getElementById('dlg');
  dlg.showModal = function () { this.open = true; };
  const answer = (v) => { dlg.returnValue = v; dlg.open = false; dlg.dispatchEvent(new w.Event('close')); };
  const cb = (id) => d.querySelector(`#items input[data-id="${id}"]`);
  const badge = (id) => cb(id).closest('.item').querySelector('.state').textContent;
  const change = (el, v) => { if (v !== undefined) el.value = v; el.dispatchEvent(new w.Event('change', { bubbles: true })); };
  return { w, d, st, calls, dlg, answer, cb, badge, change };
}

(async () => {
  let n = 0; const ok = (m) => console.log(`ok ${++n} - ${m}`);

  // 1. 初期表示
  let s = await boot();
  assert.strictEqual(s.d.querySelectorAll('#items .item').length, 4);
  assert.ok(s.cb('compress').checked && !s.cb('sendto').checked && s.cb('open:zip').checked, '登録済み/要修復はチェック済み');
  assert.strictEqual(s.badge('compress'), '登録済み');
  assert.match(s.badge('open:zip'), /要修復/);
  assert.strictEqual(s.badge('sendto'), '未登録');
  assert.strictEqual(s.d.getElementById('ver').textContent, 'v0.3.0');
  assert.strictEqual(s.d.getElementById('level').value, 'normal');
  assert.strictEqual(s.d.querySelector('input[name=append]:checked').value, 'safe');
  assert.match(s.d.getElementById('paths').textContent, /tote\.exe/);
  ok('初期表示（状態バッジ・設定値）');

  // 2. チェックを変えて適用
  s.cb('sendto').checked = true; s.cb('open:7z').checked = true; s.cb('compress').checked = false;
  s.d.getElementById('btnApply').click();
  await sleep(30);
  const wants = Object.fromEntries(s.calls.find((c) => c[0] === 'apply_items')[1].wants);
  assert.deepStrictEqual(wants, { compress: false, sendto: true, 'open:zip': true, 'open:7z': true });
  assert.strictEqual(s.badge('sendto'), '登録済み');
  assert.strictEqual(s.badge('compress'), '未登録');
  assert.strictEqual(s.badge('open:zip'), '登録済み', '要修復も適用で直る');
  ok('変更を適用 → 状態が更新される');

  // 3. 設定は即保存（圧縮レベル/追加方式/確認）
  s = await boot();
  s.change(s.d.getElementById('level'), 'best');
  await sleep(10);
  assert.strictEqual(s.calls.filter((c) => c[0] === 'save_config').pop()[1].config.compressLevel, 'best');
  const fast = s.d.querySelector('input[name=append][value=fast]');
  fast.checked = true; s.change(fast);
  s.d.getElementById('confirmRisky').checked = false; s.change(s.d.getElementById('confirmRisky'));
  s.change(s.d.getElementById('reveal'), 'never');
  await sleep(20);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(s.st.config)), { compressFormat: 'zip', compressLevel: 'best', compressSolid: true, confirmRisky: false, reveal: 'never', extractDest: 'besideArchive', appendMode: 'fast' });
  ok('設定の変更は即保存される');

  // 4. 修復
  s = await boot();
  s.d.getElementById('btnRepair').click();
  await sleep(40);
  assert.ok(s.calls.some((c) => c[0] === 'repair_items'));
  assert.strictEqual(s.badge('open:zip'), '登録済み');
  assert.match(s.d.getElementById('toastMsg').textContent, /登録し直しました/);
  ok('壊れた登録を修復');

  // 5. すべて解除は確認が要る
  s = await boot();
  s.d.getElementById('btnUninstall').click();
  await sleep(10);
  assert.ok(s.dlg.open);
  s.answer('cancel'); await sleep(10);
  assert.ok(!s.calls.some((c) => c[0] === 'uninstall_everything'), 'キャンセルなら解除しない');
  s.d.getElementById('btnUninstall').click(); await sleep(10);
  s.answer('ok'); await sleep(30);
  assert.ok(s.calls.some((c) => c[0] === 'uninstall_everything'));
  assert.strictEqual(s.badge('compress'), '未登録');
  ok('すべて解除（確認あり）');

  // 6. 設定の初期化も確認あり
  s = await boot();
  s.change(s.d.getElementById('level'), 'store'); await sleep(10);
  s.d.getElementById('btnReset').click(); await sleep(10);
  s.answer('ok'); await sleep(30);
  assert.strictEqual(s.d.getElementById('level').value, 'normal');
  assert.ok(s.calls.some((c) => c[0] === 'reset_config'));
  ok('設定を初期化');

  // 7. 適用に失敗したらエラーを出し、実際の状態を読み直す
  s = await boot({ failApply: true });
  const before = s.calls.filter((c) => c[0] === 'get_state').length;
  s.d.getElementById('btnApply').click();
  await sleep(40);
  assert.match(s.d.getElementById('toastMsg').textContent, /アクセスが拒否/);
  assert.ok(s.d.getElementById('toast').classList.contains('error'));
  assert.strictEqual(s.calls.filter((c) => c[0] === 'get_state').length, before + 1);
  ok('適用失敗 → エラー表示と状態の再取得');

  // 8. Windows以外は登録系を無効化
  s = await boot({ supported: false });
  for (const id of ['btnApply', 'btnRepair', 'btnDefaultApps', 'btnUninstall']) assert.ok(s.d.getElementById(id).disabled, id);
  assert.match(s.d.getElementById('regNote').textContent, /Windows 以外/);
  ok('Windows以外では登録ボタンが無効');

  // 9. 項目名はテキストとして出る（HTML注入されない）
  s = await boot();
  assert.strictEqual(s.d.querySelectorAll('#items img, #items script').length, 0);
  ok('項目はtextContentで描画');

  console.log(`\nall ${n} passed`);
  process.exit(0);
})().catch((e) => { console.error('FAIL:', e); process.exit(1); });
