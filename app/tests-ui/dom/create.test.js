// 「書庫を作成」ダイアログ（app/ui/create.html）の動作テスト。
const { JSDOM } = require('jsdom');
const assert = require('node:assert');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const P = (id, label) => ({ id, label });
const NORMAL = [P('store', '圧縮しない'), P('fast', '速度優先'), P('normal', '標準'), P('best', '最高')];
const formats = [
  { id: 'zip', label: 'ZIP', extension: 'zip', presets: NORMAL, supportsPassword: true, supportsSolid: false, supportsNameEncryption: false },
  { id: '7z', label: '7z', extension: '7z', presets: NORMAL, supportsPassword: true, supportsSolid: true, supportsNameEncryption: true },
  { id: 'targz', label: 'tar.gz', extension: 'tar.gz', presets: [P('fast', '速度優先'), P('normal', '標準'), P('best', '最高')], supportsPassword: false, supportsSolid: false, supportsNameEncryption: false },
  { id: 'tar', label: 'tar（圧縮なし）', extension: 'tar', presets: [], supportsPassword: false, supportsSolid: false, supportsNameEncryption: false },
];

async function boot({ init = {}, run = null, pick = 'D:\\out' } = {}) {
  const calls = [];
  const base = { formats, format: 'zip', level: 'normal', solid: true, dir: 'C:\\work', stem: 'データ', count: 2, names: ['a.txt', 'b'] };
  const invoke = async (cmd, args) => {
    calls.push([cmd, JSON.parse(JSON.stringify(args || {}))]);
    if (cmd === 'create_init') return Object.assign(base, init);
    if (cmd === 'create_pick_dir') return pick;
    if (cmd === 'create_run') return run ? run(args.req) : { status: 'ok', output: 'C:\\work\\データ.zip', files: 2, dirs: 1, skipped: 0, size: 2048 };
    return null;
  };
  const dom = await JSDOM.fromFile(require('node:path').join(__dirname, '../../ui/create.html'), {
    runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
    beforeParse(w) { w.__TAURI__ = { core: { invoke } }; },
  });
  await new Promise((r) => dom.window.addEventListener('load', r));
  await sleep(50);
  const w = dom.window, d = w.document;
  const $ = (id) => d.getElementById(id);
  $('dlg').showModal = function () { this.open = true; };
  const set = (id, v) => { $(id).value = v; $(id).dispatchEvent(new w.Event(id === 'fmt' ? 'change' : 'input', { bubbles: true })); };
  const click = (id) => { $(id).click(); return sleep(40); };
  return { w, d, $, calls, set, click, req: () => { const c = calls.find((c) => c[0] === 'create_run'); return c ? c[1].req : undefined; } };
}

(async () => {
  let n = 0; const ok = (name) => console.log(`ok ${++n} - ${name}`);

  // 1. 初期表示
  let s = await boot();
  assert.strictEqual(s.$('fmt').value, 'zip');
  assert.strictEqual(s.$('stem').value, 'データ');
  assert.strictEqual(s.$('ext').textContent, '.zip');
  assert.strictEqual(s.$('dir').textContent, 'C:\\work');
  assert.match(s.$('src').textContent, /2 個の項目: a\.txt、b/);
  assert.ok(s.$('rowSolid').hidden && !s.$('pwSec').hidden && s.$('rowNames').hidden);
  assert.strictEqual(s.$('level').value, 'normal');
  ok('初期表示（ZIP: 固体圧縮と名前暗号化は隠れる）');

  // 2. 形式で項目が変わる
  s.set('fmt', '7z');
  assert.ok(!s.$('rowSolid').hidden && !s.$('rowNames').hidden && !s.$('pwSec').hidden);
  assert.strictEqual(s.$('ext').textContent, '.7z');
  assert.strictEqual(s.$('level').value, 'normal', '選んでいたレベルは引き継ぐ');
  s.set('fmt', 'targz');
  assert.ok(s.$('pwSec').hidden && s.$('rowSolid').hidden);
  assert.deepStrictEqual([...s.$('level').options].map((o) => o.value), ['fast', 'normal', 'best']);
  assert.strictEqual(s.$('ext').textContent, '.tar.gz');
  s.set('fmt', 'tar');
  assert.ok(s.$('rowLevel').hidden, 'tarはレベルなし');
  ok('形式の切り替えで、レベル・固体圧縮・パスワード欄が出入りする');

  // 3. 作成（パスワードなし）
  s = await boot();
  s.set('fmt', '7z');
  s.set('level', 'best');
  await s.click('btnCreate');
  assert.deepStrictEqual(s.req(), { format: '7z', level: 'best', password: null, solid: true, encryptNames: false, dir: 'C:\\work', stem: 'データ', overwrite: false });
  assert.ok(s.$('form').hidden && !s.$('result').hidden);
  assert.match(s.$('resText').textContent, /C:\\work\\データ\.zip/);
  assert.match(s.$('resText').textContent, /2 個のファイル、1 個のフォルダ.*2\.0 KB/);
  assert.ok(!/パスワードを付けました/.test(s.$('resText').textContent));
  ok('作成 → 結果画面');

  // 4. パスワード: 不一致は送らない / 一致で送る / 名前暗号化はパスワードがあるときだけ
  s = await boot();
  s.set('fmt', '7z');
  assert.ok(s.$('encNames').disabled);
  s.set('pw', 'secret'); s.set('pw2', 'secreT');
  assert.ok(!s.$('encNames').disabled);
  await s.click('btnCreate');
  assert.ok(!s.req(), '不一致のときは作成しない');
  assert.match(s.$('err').textContent, /一致しません/);
  s.set('pw2', 'secret');
  s.$('encNames').checked = true;
  s.$('solid').checked = false;
  await s.click('btnCreate');
  const r = s.req();
  assert.strictEqual(r.password, 'secret');
  assert.strictEqual(r.encryptNames, true);
  assert.strictEqual(r.solid, false);
  assert.match(s.$('resText').textContent, /パスワードを付けました/);
  ok('パスワードの確認入力・名前暗号化・固体圧縮オフ');

  // 5. パスワードを消すと名前暗号化は外れる。表示切り替え
  s = await boot();
  s.set('fmt', '7z');
  s.set('pw', 'x'); s.$('encNames').checked = true;
  s.set('pw', '');
  assert.ok(s.$('encNames').disabled && !s.$('encNames').checked);
  s.$('showPw').checked = true; s.$('showPw').dispatchEvent(new s.w.Event('change'));
  assert.strictEqual(s.$('pw').type, 'text');
  assert.strictEqual(s.$('pw2').type, 'text');
  ok('パスワードを消すと名前暗号化も外れる／表示切り替え');

  // 6. 入力チェック
  s = await boot();
  for (const [stem, re] of [['', /名前を入力/], ['a/b', /使えない文字/], ['x.', /末尾/]]) {
    s.set('stem', stem);
    await s.click('btnCreate');
    assert.match(s.$('err').textContent, re, stem);
  }
  assert.ok(!s.req());
  ok('名前の入力チェック');

  // 7. 同名ファイル → 確認 → 上書きで再実行
  let count = 0;
  s = await boot({ run: (req) => (++count === 1 ? { status: 'exists', output: 'C:\\work\\データ.zip' } : { status: 'ok', output: 'C:\\work\\データ.zip', files: 1, dirs: 0, skipped: 2, size: 10 }) });
  await s.click('btnCreate');
  assert.ok(s.$('dlg').hasAttribute('open') || s.$('dlg').open, '確認ダイアログが出る');
  assert.match(s.$('dlgMsg').textContent, /同じ名前/);
  { const dl = s.$('dlg'); dl.returnValue = 'ok'; dl.open = false; dl.dispatchEvent(new s.w.Event('close')); }
  await sleep(80);
  const runs = s.calls.filter((c) => c[0] === 'create_run');
  assert.strictEqual(runs.length, 2);
  assert.strictEqual(runs[1][1].req.overwrite, true);
  assert.match(s.$('resText').textContent, /2 個の項目.*スキップ/);
  ok('同名があれば確認してから上書き');

  // 8. エラーは画面に出て、やり直せる
  s = await boot({ run: () => { throw '入力が別々のフォルダにあります'; } });
  await s.click('btnCreate');
  assert.match(s.$('err').textContent, /別々のフォルダ/);
  assert.ok(!s.$('btnCreate').disabled && !s.$('form').hidden);
  ok('失敗したらエラー表示、ボタンは戻る');

  // 9. 保存先の変更 / 保存先なし
  s = await boot();
  await s.click('btnDir');
  assert.strictEqual(s.$('dir').textContent, 'D:\\out');
  await s.click('btnCreate');
  assert.strictEqual(s.req().dir, 'D:\\out');
  s = await boot({ init: { dir: '' } });
  await s.click('btnCreate');
  assert.match(s.$('err').textContent, /保存先/);
  assert.ok(!s.req());
  ok('保存先の変更／保存先が決まらないときは作らない');

  // 10. 保存済みの形式・レベル・固体圧縮が初期値になる
  s = await boot({ init: { format: '7z', level: 'fast', solid: false } });
  assert.strictEqual(s.$('fmt').value, '7z');
  assert.strictEqual(s.$('level').value, 'fast');
  assert.strictEqual(s.$('solid').checked, false);
  ok('前回の選択が初期値');

  // 11. 結果画面のボタン
  s = await boot();
  await s.click('btnCreate');
  await s.click('btnReveal');
  assert.deepStrictEqual(s.calls.find((c) => c[0] === 'create_reveal')[1], { path: 'C:\\work\\データ.zip' });
  await s.click('btnClose');
  assert.ok(s.calls.some((c) => c[0] === 'create_close'));
  ok('フォルダで表示／閉じる');

  console.log(`\nall ${n} passed`);
  process.exit(0);
})().catch((e) => { console.error('FAIL:', e); process.exit(1); });
