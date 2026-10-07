// 画面（app/ui/index.html）の動作テスト。jsdom が必要: `cd app/tests-ui && npm install && npm run test:dom`
const { JSDOM } = require('jsdom');
const assert = require('node:assert');

const E = (path, o = {}) => ({ index: 0, path, isDir: false, size: 100, packed: 50, modified: '2020-05-17 12:34',
  crc32: 0, method: 'Deflated', encrypted: false, safe: true, risky: false, symlink: false, ...o });
const entries = [
  E('docs', { isDir: true }), E('docs/guide.md', { size: 3000 }), E('docs/img', { isDir: true }), E('docs/img/a.png', { size: 90000 }),
  E('readme.txt', { size: 10 }), E('setup.exe', { size: 500000, risky: true }),
  E('<img src=x onerror=window.__pwned=1>.txt'), E('../evil.txt', { safe: false }), E('secret.bin', { encrypted: true }),
];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function boot({ prepareDelay = 0, cfg = {}, extractAll = { dest: 'C:\\x\\test', files: 3, skipped: [] }, ents = entries, format = 'ZIP', added = { files: 2, dirs: 0, replaced: 0, skipped: 0, fellBack: false } } = {}) {
  const calls = [];
  const handlers = {};
  let extra = [];
  const invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'load_archive') return { path: 'C:\\x\\test.zip', name: 'test.zip', info: { entries: [...ents, ...extra], totalSize: 0, totalPacked: 0, comment: '', format } };
    if (cmd === 'get_config') return Object.assign({ confirmRisky: true, extractDest: 'besideArchive' }, cfg);
    if (cmd === 'prepare_drag') { await sleep(prepareDelay); return null; }
    if (cmd === 'extract_all') return extractAll;
    if (cmd === 'add_files') { extra = [E('added.txt')]; return added; }
    return null;
  };
  const dom = await JSDOM.fromFile(require('node:path').join(__dirname, '../../ui/index.html'), {
    runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
    beforeParse(w) { w.__TAURI__ = { core: { invoke }, event: { listen: async (n, f) => { handlers[n] = f; } } }; },
  });
  await new Promise((r) => dom.window.addEventListener('load', r));
  await sleep(50);
  const w = dom.window, d = w.document;
  const fire = (target, type, p = {}) => {
    const ev = new w.MouseEvent(type, { bubbles: true, cancelable: true, clientX: p.x || 0, clientY: p.y || 0, button: 0, ctrlKey: !!p.ctrl, shiftKey: !!p.shift });
    Object.defineProperty(ev, 'pointerId', { value: 1 });
    target.dispatchEvent(ev);
  };
  const rowsText = () => [...d.querySelectorAll('#rows tr')].map((tr) => tr.querySelector('.label').textContent);
  const row = (name) => [...d.querySelectorAll('#rows tr')].find((tr) => tr.querySelector('.label').textContent === name);
  const emit = (n, payload) => handlers[n] && handlers[n]({ payload });
  return { w, d, calls, fire, rowsText, row, emit };
}

(async () => {
  let n = 0; const ok = (name) => console.log(`ok ${++n} - ${name}`);

  // 1. 初期表示
  let s = await boot();
  assert.ok(s.rowsText().includes('docs') && s.rowsText().indexOf('docs') < s.rowsText().indexOf('readme.txt'), 'フォルダが先頭');
  assert.ok(s.rowsText().includes('readme.txt'));
  ok('初期表示: ルートの項目とフォルダ先頭');

  // 2. 警告バナー
  const warn = s.d.getElementById('warn');
  assert.ok(!warn.hidden);
  assert.match(warn.textContent, /実行形式・スクリプトが 1 個/);
  assert.match(warn.textContent, /不正なパス.*1 個/);
  assert.match(warn.textContent, /パスワード付き.*1 個/);
  ok('警告バナー（実行形式/不正パス/暗号化）');

  // 3. XSS: 名前はテキストとして出る
  assert.strictEqual(s.d.querySelectorAll('#rows img').length, 0);
  assert.ok(s.rowsText().includes('<img src=x onerror=window.__pwned=1>.txt'));
  assert.strictEqual(s.w.__pwned, undefined);
  ok('悪意あるファイル名がHTMLとして解釈されない');

  // 4. フォルダ移動
  s.row('docs').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  assert.deepStrictEqual(s.rowsText().filter((x) => x !== '..'), ['img', 'guide.md']);
  assert.strictEqual(s.rowsText()[0], '..');
  assert.deepStrictEqual([...s.d.querySelectorAll('#crumbs button')].map((b) => b.textContent), ['test.zip', 'docs']);
  s.w.dispatchEvent(new s.w.KeyboardEvent('keydown', { key: 'Backspace', bubbles: true }));
  assert.ok(s.rowsText().includes('readme.txt'));
  ok('フォルダへ入る/Backspaceで戻る/パンくず');

  // 5. 並べ替え
  const th = (k) => s.d.querySelector(`th[data-key="${k}"]`);
  th('size').click();
  const files = () => s.rowsText().filter((x) => !['docs'].includes(x));
  assert.strictEqual(files()[0], '<img src=x onerror=window.__pwned=1>.txt'.length ? files()[0] : '');
  const sizeAsc = files();
  th('size').click();
  assert.deepStrictEqual(files(), [...sizeAsc].reverse().length ? files() : []);
  assert.strictEqual(s.rowsText()[0], 'docs', '降順でもフォルダ先頭');
  assert.notDeepStrictEqual(files(), sizeAsc, '2回目のクリックで逆順');
  ok('列見出しで並べ替え（昇降、フォルダ先頭維持）');

  // 6. 選択
  s = await boot();
  s.fire(s.row('readme.txt'), 'pointerdown');
  s.fire(s.w, 'pointerup');
  s.fire(s.row('setup.exe'), 'pointerdown', { ctrl: true });
  s.fire(s.w, 'pointerup');
  assert.strictEqual(s.d.querySelectorAll('#rows tr.sel').length, 2);
  assert.match(s.d.getElementById('stSel').textContent, /2 項目を選択/);
  s.fire(s.row('docs'), 'pointerdown');
  s.fire(s.w, 'pointerup');
  s.fire(s.row('setup.exe'), 'pointerdown', { shift: true });
  s.fire(s.w, 'pointerup');
  assert.ok(s.d.querySelectorAll('#rows tr.sel').length >= 3, 'Shiftで範囲選択');
  ok('Ctrl/Shift選択とステータス表示');

  // 7. 複数選択中の1行を（ドラッグせず）クリック → 1つに絞る
  s = await boot();
  s.fire(s.row('readme.txt'), 'pointerdown');
  s.fire(s.row('setup.exe'), 'pointerdown', { ctrl: true });
  s.fire(s.w, 'pointerup');
  s.fire(s.row('setup.exe'), 'pointerdown');
  s.fire(s.w, 'pointerup');
  assert.deepStrictEqual([...s.d.querySelectorAll('#rows tr.sel .label')].map((e) => e.textContent), ['setup.exe']);
  ok('複数選択中にクリックで1つに絞る');

  // 8. ドラッグ開始 → prepare_drag → start_drag
  s = await boot();
  s.fire(s.row('readme.txt'), 'pointerdown', { x: 10, y: 10 });
  s.fire(s.w, 'pointermove', { x: 30, y: 10 });
  await sleep(30);
  const cmds = s.calls.map((c) => c[0]).filter((c) => c.endsWith('_drag'));
  assert.deepStrictEqual(cmds, ['prepare_drag', 'start_drag']);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(s.calls.find((c) => c[0] === 'start_drag')[1])), { paths: ['readme.txt'] });
  ok('ドラッグ開始で prepare_drag → start_drag（選択パスを渡す）');

  // 9. 小さな動きではドラッグにならない
  s = await boot();
  s.fire(s.row('readme.txt'), 'pointerdown', { x: 10, y: 10 });
  s.fire(s.w, 'pointermove', { x: 12, y: 11 });
  s.fire(s.w, 'pointerup');
  await sleep(20);
  assert.strictEqual(s.calls.filter((c) => c[0].endsWith('_drag')).length, 0);
  ok('数pxの揺れではドラッグ扱いにしない');

  // 10. 展開待ちの間にボタンを離したら start_drag しない
  s = await boot({ prepareDelay: 80 });
  s.fire(s.row('readme.txt'), 'pointerdown', { x: 10, y: 10 });
  s.fire(s.w, 'pointermove', { x: 40, y: 10 });
  s.fire(s.w, 'pointerup');
  await sleep(150);
  assert.deepStrictEqual(s.calls.map((c) => c[0]).filter((c) => c.endsWith('_drag')), ['prepare_drag']);
  assert.match(s.d.getElementById('toastMsg').textContent, /もう一度ドラッグ/);
  ok('展開中にボタンを離したら開始せず、案内を出す');

  // 11. 実行形式は確認ダイアログを経てから開く
  s = await boot();
  const dlg = s.d.getElementById('dlg');
  dlg.showModal = function () { this.open = true; };
  s.row('setup.exe').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  await sleep(10);
  assert.ok(dlg.open, '確認ダイアログが出る');
  assert.strictEqual(s.calls.filter((c) => c[0] === 'open_entry').length, 0, '確認前は開かない');
  dlg.returnValue = 'ok'; dlg.open = false; dlg.dispatchEvent(new s.w.Event('close'));
  await sleep(20);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(s.calls.find((c) => c[0] === 'open_entry')[1])), { path: 'setup.exe' });
  ok('実行形式は確認してから開く');

  s = await boot();
  // 12. 暗号化・不正パスは開かない
  s.row('secret.bin').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  s.row('..').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  s.row('evil.txt').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  await sleep(10);
  assert.strictEqual(s.calls.filter((c) => c[0] === 'open_entry').length, 0);
  ok('パスワード付き/不正パスは開かない');

  // 13. すべて展開
  s = await boot();
  s.d.getElementById('btnExtractAll').click();
  await sleep(30);
  assert.ok(s.calls.some((c) => c[0] === 'extract_all'));
  assert.match(s.d.getElementById('toastMsg').textContent, /3 個のファイルを展開しました/);
  assert.ok(!s.d.getElementById('toastAct').hidden, '展開先を開くボタン');
  ok('すべて展開 → 結果通知と「展開先を開く」');

  // 14. 選択なしで「選択を展開」
  s = await boot();
  s.d.getElementById('btnExtractSel').click();
  assert.match(s.d.getElementById('toastMsg').textContent, /選択してください/);
  assert.ok(!s.calls.some((c) => c[0] === 'extract_selected'));
  ok('選択なしの「選択を展開」は案内だけ');

  // 15. 確認OFFの設定なら、実行形式もそのまま開く
  s = await boot({ cfg: { confirmRisky: false } });
  const dlg2 = s.d.getElementById('dlg');
  dlg2.showModal = function () { this.open = true; };
  s.row('setup.exe').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  await sleep(20);
  assert.ok(!dlg2.open, '確認ダイアログは出ない');
  assert.ok(s.calls.some((c) => c[0] === 'open_entry'));
  ok('設定で確認OFFなら確認なしで開く');

  // 16. 展開先を毎回選ぶ設定でキャンセルしたら、何も言わずに戻る
  s = await boot({ cfg: { extractDest: 'ask' }, extractAll: null });
  s.d.getElementById('btnExtractAll').click();
  await sleep(30);
  assert.ok(s.d.getElementById('toast').hidden, 'トーストは出ない');
  assert.ok(!s.d.getElementById('btnExtractAll').disabled, 'ボタンは戻る');
  ok('展開先の選択をキャンセルしたら何も起きない');

  // 17. 固体形式（格納サイズ不明=0）は格納列が空欄、形式名がステータスに出る
  s = await boot({ ents: [E('a.txt', { size: 100, packed: 0 }), E('b.txt', { size: 100, packed: 40 })], format: 'tar.gz' });
  const cells = (name) => [...s.row(name).querySelectorAll('td')].map((t) => t.textContent);
  assert.strictEqual(cells('a.txt')[2], '');
  assert.strictEqual(cells('b.txt')[2], '40');
  assert.match(s.d.getElementById('stInfo').textContent, /tar\.gz/);
  ok('格納サイズ不明は空欄、形式名を表示');

  // 18. ⚙ で設定画面を開く
  s = await boot();
  s.d.getElementById('btnSettings').click();
  await sleep(10);
  assert.ok(s.calls.some((c) => c[0] === 'open_settings'));
  ok('⚙で設定画面を開く');

  // 19. ドロップで追加（ZIP）
  s = await boot();
  s.emit('tauri://drag-enter', { paths: ['C:\\in\\a.txt'] });
  assert.ok(!s.d.getElementById('drop').hidden);
  assert.match(s.d.getElementById('dropBox').textContent, /ここにドロップして追加/);
  s.row('docs').dispatchEvent(new s.w.MouseEvent('dblclick', { bubbles: true }));
  s.emit('tauri://drag-drop', { paths: ['C:\\in\\a.txt', 'C:\\in\\dir'] });
  assert.ok(s.d.getElementById('drop').hidden, 'ドロップしたらオーバーレイは消える');
  await sleep(40);
  const add = s.calls.find((c) => c[0] === 'add_files');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(add[1])), { paths: ['C:\\in\\a.txt', 'C:\\in\\dir'], dest: 'docs' });
  assert.strictEqual(s.calls.filter((c) => c[0] === 'load_archive').length, 2, '追加後に再読み込み');
  assert.match(s.d.getElementById('toastMsg').textContent, /2 個のファイルを追加しました/);
  assert.strictEqual(s.rowsText()[0], '..', '今いるフォルダに留まる');
  ok('ZIPへドロップ→現在のフォルダへ追加→再読み込み');

  // 20. ZIP以外には追加できない / 自分でドラッグ出ししたものは無視
  s = await boot({ format: '7z' });
  s.emit('tauri://drag-enter', { paths: ['C:\\in\\a.txt'] });
  assert.ok(s.d.getElementById('drop').classList.contains('deny'));
  assert.match(s.d.getElementById('dropBox').textContent, /追加できません/);
  s.emit('tauri://drag-drop', { paths: ['C:\\in\\a.txt'] });
  assert.ok(!s.calls.some((c) => c[0] === 'add_files'));
  assert.match(s.d.getElementById('toastMsg').textContent, /ZIPのみ/);
  s = await boot();
  s.emit('tauri://drag-enter', { paths: ['C:\\Temp\\tote-drag\\123\\a.txt'] });
  assert.ok(s.d.getElementById('drop').hidden);
  s.emit('tauri://drag-drop', { paths: ['C:\\Temp\\tote-drag\\123\\a.txt'] });
  await sleep(20);
  assert.ok(!s.calls.some((c) => c[0] === 'add_files'));
  ok('ZIP以外は拒否表示、自分のドラッグ出しは無視');

  // 21. 置き換え・方式切替の通知
  s = await boot({ added: { files: 1, dirs: 0, replaced: 1, skipped: 0, fellBack: true } });
  s.emit('tauri://drag-drop', { paths: ['C:\\in\\a.txt'] });
  await sleep(40);
  assert.match(s.d.getElementById('toastMsg').textContent, /1 個を置き換え.*安全な方式/);
  ok('置き換え・安全な方式への切替を通知');

  console.log(`\nall ${n} passed`);
  process.exit(0);
})().catch((e) => { console.error('FAIL:', e); process.exit(1); });
