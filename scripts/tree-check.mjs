import assert from 'node:assert/strict';
import { treeRows } from '../src/tree.ts';
const hits = [
  { path: 'C:\\a\\a1.txt', name: 'a1.txt' },
  { path: 'C:\\a\\nested\\中文😀.pdf', name: '中文😀.pdf' },
  { path: 'D:\\a\\a1.txt', name: 'a1.txt' },
  { path: '\\\\server\\share\\a\\note.md', name: 'note.md' },
];
const all = treeRows(hits, new Set());
assert.equal(new Set(all.map(row => row.key)).size, all.length);
assert.deepEqual(all.filter(row => row.hit !== null).map(row => row.hit).sort(), [0, 1, 2, 3]);
const folder = all.find(row => row.key === 'folder:C:\\a\\');
assert.equal(folder.count, 2);
assert.equal(folder.depth, 2);
const collapsed = treeRows(hits, new Set([folder.key]));
assert.equal(collapsed.find(row => row.key === folder.key).count, 2);
assert.deepEqual(collapsed.filter(row => row.hit !== null).map(row => row.hit).sort(), [2, 3]);
assert.equal(all.find(row => row.hit === 1).depth, 4);
assert.equal(all.find(row => row.hit === 3).parent, 'folder:\\\\server\\share\\a\\');
assert.deepEqual(treeRows([], new Set()), []);
console.log('tree grouping, independent roots, counts, nesting, collapse and Unicode: passed');
