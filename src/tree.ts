export type TreeHit = { path: string; name: string };
export type TreeRow = { key: string; name: string; parent: string; depth: number; count: number; hit: number | null; position: number; siblings: number };
type Branch = { key: string; name: string; children: Map<string, Branch>; files: number[]; count: number };

export function treeRows(hits: TreeHit[], collapsed: Set<string>): TreeRow[] {
  const root: Branch = { key: "", name: "", children: new Map(), files: [], count: 0 };
  hits.forEach((hit, index) => {
    const parts = hit.path.replaceAll("/", "\\").split("\\");
    parts.pop();
    let branch = root;
    let path = hit.path.startsWith("\\\\") ? "\\\\" : "";
    for (const name of parts.filter(Boolean)) {
      path += name + "\\";
      const key = "folder:" + path;
      let child = branch.children.get(key);
      if (!child) { child = { key, name, children: new Map(), files: [], count: 0 }; branch.children.set(key, child); }
      child.count++; branch = child;
    }
    branch.files.push(index);
  });
  const rows: TreeRow[] = [];
  function visit(branch: Branch, depth: number) {
    const folders = [...branch.children.values()].sort((a, b) => a.name.localeCompare(b.name, "zh-CN"));
    const files = branch.files.sort((a, b) => hits[a].name.localeCompare(hits[b].name, "zh-CN"));
    const siblings = folders.length + files.length;
    folders.forEach((child, index) => {
      rows.push({ key: child.key, name: child.name, parent: branch.key, depth, count: child.count, hit: null, position: index + 1, siblings });
      if (!collapsed.has(child.key)) visit(child, depth + 1);
    });
    files.forEach((index, position) => rows.push({ key: "file:" + hits[index].path, name: hits[index].name, parent: branch.key, depth, count: 1, hit: index, position: folders.length + position + 1, siblings }));
  }
  visit(root, 1);
  return rows;
}
