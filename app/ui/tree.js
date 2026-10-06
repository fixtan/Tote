// ZIPのエントリ一覧 → フォルダ階層。DOMに触れない純粋な処理だけを置く（Nodeでテストできる）。
(function (root) {
  'use strict';

  /** entries: [{path:'a/b.txt', isDir, size, packed, modified, ...}] → ルートノード */
  function buildTree(entries) {
    const top = { name: '', path: '', isDir: true, children: new Map(), entry: null };
    for (const e of entries) {
      const comps = e.path.split('/').filter(Boolean);
      if (!comps.length) continue;
      let node = top;
      comps.forEach((c, i) => {
        const last = i === comps.length - 1;
        let child = node.children.get(c);
        if (!child) {
          child = {
            name: c,
            path: comps.slice(0, i + 1).join('/'),
            isDir: !last || e.isDir,
            children: new Map(),
            entry: null,
          };
          node.children.set(c, child);
        }
        if (last) {
          child.entry = e;
          child.isDir = child.isDir || e.isDir;
        } else {
          child.isDir = true; // ZIPにフォルダ項目が無くても、中身があればフォルダ
        }
        node = child;
      });
    }
    return top;
  }

  function extOf(name) {
    const i = name.lastIndexOf('.');
    return i > 0 && i < name.length - 1 ? name.slice(i + 1).toUpperCase() : '';
  }

  function typeLabel(node) {
    if (node.isDir) return 'ファイル フォルダー';
    const ext = extOf(node.name);
    return ext ? ext + ' ファイル' : 'ファイル';
  }

  const collator = new Intl.Collator('ja', { numeric: true, sensitivity: 'base' });

  /** フォルダを常に先頭にして、key で並べ替える比較関数。 */
  function compareNodes(key, dir) {
    const val = (n) => {
      switch (key) {
        case 'size': return n.entry ? n.entry.size : -1;
        case 'packed': return n.entry ? n.entry.packed : -1;
        case 'modified': return n.entry && n.entry.modified ? n.entry.modified : '';
        case 'type': return typeLabel(n);
        default: return null;
      }
    };
    return (a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      let r = 0;
      if (key !== 'name') {
        const x = val(a), y = val(b);
        r = typeof x === 'number' ? x - y : collator.compare(x, y);
      }
      if (r === 0) r = collator.compare(a.name, b.name);
      else r *= dir;
      return r;
    };
  }

  function fmtBytes(n) {
    if (n < 1024) return n + ' B';
    const units = ['KB', 'MB', 'GB', 'TB'];
    let i = -1;
    do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
    return (n >= 100 ? n.toFixed(0) : n.toFixed(1)) + ' ' + units[i];
  }

  function stats(entries, rootNode) {
    let files = 0, risky = 0, unsafe = 0, encrypted = 0, total = 0, packed = 0;
    for (const e of entries) {
      if (!e.isDir) { files++; total += e.size; packed += e.packed; }
      if (e.risky) risky++;
      if (!e.safe) unsafe++;
      if (e.encrypted && !e.isDir) encrypted++;
    }
    let dirs = 0;
    const walk = (n) => { for (const c of n.children.values()) { if (c.isDir) { dirs++; walk(c); } } };
    walk(rootNode);
    return { files, dirs, risky, unsafe, encrypted, total, packed };
  }

  const api = { buildTree, compareNodes, fmtBytes, typeLabel, extOf, stats };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.ToteTree = api;
})(typeof window !== 'undefined' ? window : globalThis);
