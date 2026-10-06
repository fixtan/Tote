// 実行: node --test app/tests-ui/tree.test.js
const test = require('node:test');
const assert = require('node:assert');
const T = require('../ui/tree.js');

const E = (path, o = {}) => ({ index: 0, path, isDir: false, size: 1, packed: 1, modified: null, crc32: 0,
  method: 'Stored', encrypted: false, safe: true, risky: false, symlink: false, ...o });

test('フォルダ項目が無くても、中身から階層を作る', () => {
  const root = T.buildTree([E('a/b/c.txt'), E('top.txt')]);
  assert.deepStrictEqual([...root.children.keys()].sort(), ['a', 'top.txt']);
  assert.strictEqual(root.children.get('a').isDir, true);
  assert.strictEqual(root.children.get('a').children.get('b').children.get('c.txt').isDir, false);
});

test('明示的な空フォルダはフォルダとして残る', () => {
  const root = T.buildTree([E('empty', { isDir: true })]);
  assert.strictEqual(root.children.get('empty').isDir, true);
  assert.strictEqual(root.children.get('empty').children.size, 0);
});

test('フォルダが常に先頭、名前は自然順', () => {
  const root = T.buildTree([E('b.txt'), E('file10.txt'), E('file2.txt'), E('zdir/x', {}), E('A.txt')]);
  const names = [...root.children.values()].sort(T.compareNodes('name', 1)).map((n) => n.name);
  assert.deepStrictEqual(names, ['zdir', 'A.txt', 'b.txt', 'file2.txt', 'file10.txt']);
  const desc = [...root.children.values()].sort(T.compareNodes('name', -1)).map((n) => n.name);
  assert.strictEqual(desc[0], 'zdir', '降順でもフォルダが先頭');
});

test('サイズ順と降順', () => {
  const root = T.buildTree([E('s.txt', { size: 5 }), E('m.txt', { size: 50 }), E('l.txt', { size: 500 })]);
  const asc = [...root.children.values()].sort(T.compareNodes('size', 1)).map((n) => n.name);
  const desc = [...root.children.values()].sort(T.compareNodes('size', -1)).map((n) => n.name);
  assert.deepStrictEqual(asc, ['s.txt', 'm.txt', 'l.txt']);
  assert.deepStrictEqual(desc, ['l.txt', 'm.txt', 's.txt']);
});

test('種類ラベル', () => {
  assert.strictEqual(T.typeLabel({ name: 'a.png', isDir: false }), 'PNG ファイル');
  assert.strictEqual(T.typeLabel({ name: 'README', isDir: false }), 'ファイル');
  assert.strictEqual(T.typeLabel({ name: '.gitignore', isDir: false }), 'ファイル');
  assert.strictEqual(T.typeLabel({ name: 'd', isDir: true }), 'ファイル フォルダー');
});

test('バイト表記', () => {
  assert.strictEqual(T.fmtBytes(512), '512 B');
  assert.strictEqual(T.fmtBytes(1536), '1.5 KB');
  assert.strictEqual(T.fmtBytes(5 * 1024 * 1024), '5.0 MB');
});

test('統計: ファイル数・フォルダ数・警告の件数', () => {
  const entries = [E('a/x.exe', { risky: true, size: 10, packed: 4 }), E('b.txt', { size: 5, packed: 3 }),
    E('../evil', { safe: false }), E('s.bin', { encrypted: true })];
  const root = T.buildTree(entries);
  const s = T.stats(entries, root);
  assert.strictEqual(s.files, 4);
  assert.strictEqual(s.risky, 1);
  assert.strictEqual(s.unsafe, 1);
  assert.strictEqual(s.encrypted, 1);
  assert.strictEqual(s.total, 10 + 5 + 1 + 1);
});
