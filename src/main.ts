import { invoke } from "@tauri-apps/api/core";
import { Virtualizer, observeElementRect, observeElementOffset, elementScroll, defaultRangeExtractor } from "@tanstack/virtual-core";
import "./style.css";
import { treeRows, type TreeRow } from "./tree";

type Hit = { id: number; path: string; name: string; size: number };
type Page = { request: number; offset: number; hits: Hit[]; found: number; complete: boolean; engine_ms: number; read_ms: number; verify_ms: number };
type Part = { text: string; matched: boolean };
type Snippet = { location: number; kind: "line" | "paragraph" | "page"; parts: Part[] };
type Preview = { request: number; id: number; snippets: Snippet[] };
type Status = { default_extensions: string[]; extensions: string[]; available_extensions: string[]; phase: string; ready: boolean; complete: boolean; indexed: number; checked: number; failed: number; last_updated: number; version: number; roots: string[]; exclusions: string[]; failures: { path: string; reason: string }[]; message: string };
type Statistics = { total: number; source_bytes: number; text: number; docx: number; pdf: number };
const el = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const query = el<HTMLInputElement>("query");
const scroller = el("results");
const items = el("result-items");
const message = el("result-message");
let request = Date.now() * 1000;
let hits: Hit[] = [];
let complete = true;
let selected = -1;
let rows: TreeRow[] = [];
let activeKey = "";
const collapsed = new Set<string>();
let configuredTypes: string[] = [];
let availableTypes: string[] = [];
let defaultTypes: string[] = [];
let submittingRefresh = false;
let engineBusy = false;
let canCancel = false;
const resultType = el<HTMLSelectElement>("result-type");
let composing = false;
let compositionValue: string | null = null;
let paging = false;
let querying = false;
let queuedQuery: { request: number; query: string; extensions: string[] | null } | null = null;
let summaryBusy = false;
let version = 0;
let started = 0;
let painted = false;
let ready = false;
let timings = { engineMs: 0, readMs: 0, verifyMs: 0, searchRoundTripMs: 0, summaryRoundTripMs: 0 };
const summaries = new Map<number, Snippet[]>();
const failedSummaries = new Set<number>();
const notify = (text: unknown) => { el("notice").textContent = String(text); };
const button = (id: string) => el<HTMLButtonElement>(id);
function textParts(target: HTMLElement, parts: Part[]) {
  target.replaceChildren(...parts.map(part => {
    const node = document.createElement(part.matched ? "mark" : "span");
    node.textContent = part.text;
    return node;
  }));
}
const virtual = new Virtualizer<HTMLDivElement, HTMLDivElement>({
  count: 0, getScrollElement: () => scroller as HTMLDivElement, estimateSize: index => rows[index]?.hit === null ? 32 : 94,
  getItemKey: index => rows[index]?.key ?? index,
  scrollToFn: elementScroll, observeElementRect, observeElementOffset, overscan: 4,
  rangeExtractor: range => {
    const indexes = defaultRangeExtractor(range);
    const active = rows.findIndex(row => row.key === activeKey);
    if (active >= 0 && !indexes.includes(active)) indexes.push(active);
    return indexes.sort((a, b) => a - b);
  },
  onChange: () => render(),
});
virtual._didMount();
virtual._willUpdate();
function updateVirtual() {
  rows = treeRows(hits, collapsed);
  if (activeKey && !rows.some(row => row.key === activeKey)) activeKey = "";
  virtual.setOptions({ ...virtual.options, count: rows.length });
  virtual._willUpdate();
  render();
}
function treeAttributes(node: HTMLElement, row: TreeRow) {
  node.setAttribute("aria-level", String(row.depth));
  node.setAttribute("aria-posinset", String(row.position));
  node.setAttribute("aria-setsize", String(complete ? row.siblings : -1));
  node.style.paddingLeft = (12 + Math.min(row.depth - 1, 12) * 12) + "px";
}
function toggleFolder(row: TreeRow) {
  if (collapsed.has(row.key)) collapsed.delete(row.key); else collapsed.add(row.key);
  updateVirtual();
}
function visibleHitIds() {
  return virtual.getVirtualItems().flatMap(row => rows[row.index]?.hit === null || !rows[row.index] ? [] : [hits[rows[row.index].hit!].id]);
}
function revealFirstHit() {
  if (painted || !hits.length) return;
  const visible = virtual.getVirtualItems().some(row => row.end > scroller.scrollTop && row.start < scroller.scrollTop + scroller.clientHeight && rows[row.index].hit !== null);
  const first = rows.findIndex(row => row.hit !== null);
  if (!visible && first >= 0) virtual.scrollToIndex(first, { align: "end" });
}
function paintReady() {
  if (painted || !started) return;
  const visible = virtual.getVirtualItems().filter(row => row.end > scroller.scrollTop && row.start < scroller.scrollTop + scroller.clientHeight);
  if (hits.length && !visible.some(row => rows[row.index].hit !== null)) return;
  if ((hits.length === 0 && !complete) || visible.some(row => rows[row.index].hit !== null && !summaries.has(hits[rows[row.index].hit!].id))) return;
  painted = true;
  const current = request;
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (request !== current) return;
    window.dispatchEvent(new CustomEvent("search-painted", { detail: { request: current, milliseconds: performance.now() - started, visible: visible.length, ...timings } }));
  }));
}
function render() {
  items.style.height = virtual.getTotalSize() + "px";
  const virtualRows = virtual.getVirtualItems();
  items.replaceChildren(...virtualRows.map(row => {
    const entry = rows[row.index];
    if (entry.hit === null) {
      const node = document.createElement("div");
      node.className = "tree-folder"; node.id = "tree-row-" + row.index;
      node.setAttribute("role", "treeitem");
      node.setAttribute("aria-expanded", String(!collapsed.has(entry.key)));
      node.setAttribute("aria-selected", String(activeKey === entry.key));
      treeAttributes(node, entry);
      node.style.transform = "translateY(" + row.start + "px)";
      node.textContent = (collapsed.has(entry.key) ? "▸ " : "▾ ") + entry.name + " (" + entry.count + (complete ? "" : "+") + ")";
      node.title = entry.key.slice(7) + " · 当前已加载的匹配文件数";
      node.addEventListener("click", () => { scroller.focus(); activeKey = entry.key; toggleFolder(entry); });
      return node;
    }
    const hit = hits[entry.hit];
    const node = document.createElement("div");
    node.className = "result"; node.id = "tree-row-" + row.index;
    node.dataset.id = String(hit.id);
    node.setAttribute("role", "treeitem");
    node.setAttribute("aria-selected", String(activeKey === entry.key));
    treeAttributes(node, entry);
    node.style.transform = "translateY(" + row.start + "px)";
    const title = document.createElement("div"); title.className = "result-title";
    const type = document.createElement("span"); type.className = "type"; type.textContent = hit.name.split(".").at(-1)?.toUpperCase() ?? "TXT";
    const name = document.createElement("span"); name.textContent = hit.name; title.append(type, name);
    const path = document.createElement("p"); path.className = "result-path"; path.textContent = hit.path; path.title = hit.path;
    const summary = document.createElement("div"); summary.className = "result-summary";
    const snippet = summaries.get(hit.id)?.[0];
    if (snippet) textParts(summary, snippet.parts);
    else summary.textContent = failedSummaries.has(hit.id) ? "摘要加载失败，请重新搜索" : "正在加载摘要…";
    node.append(title, path, summary);
    node.addEventListener("click", () => { scroller.focus(); activeKey = entry.key; void select(entry.hit!); });
    return node;
  }));
  const active = rows.findIndex(row => row.key === activeKey);
  if (active >= 0) scroller.setAttribute("aria-activedescendant", "tree-row-" + active);
  else scroller.removeAttribute("aria-activedescendant");
  el("count").textContent = hits.length ? hits.length.toLocaleString() + (complete ? " 个文件" : "+ 个文件") : complete ? "0 个文件" : "正在查找";
  button("more").hidden = complete;
  button("more").disabled = paging;
  message.hidden = hits.length > 0;
  if (query.value && hits.length === 0) message.textContent = complete ? "没有找到包含这段文字的文件。" : "正在筛选完整文字匹配…";
  void loadSummaries();
  const visibleLast = virtualRows.filter(r => r.start < scroller.scrollTop + scroller.clientHeight).at(-1)?.index ?? -1;
  if (!complete && !paging && (hits.length === 0 || (visibleHitIds().length > 0 && visibleLast >= rows.length - 8))) void next();
  paintReady();
}
async function loadSummaries() {
  if (summaryBusy) return;
  const ids = visibleHitIds().filter(id => !summaries.has(id) && !failedSummaries.has(id)).slice(0, 24);
  if (!ids.length) return;
  summaryBusy = true;
  const current = request;
  try {
    const before = performance.now();
    const data = await invoke<Preview[]>("previews", { request: current, ids, detail: false });
    if (current !== request) return;
    if (!painted) timings.summaryRoundTripMs += performance.now() - before;
    for (const preview of data) if (preview.request === current) summaries.set(preview.id, preview.snippets);
    const protectedIds = new Set(visibleHitIds());
    for (const id of summaries.keys()) { if (summaries.size <= 200) break; if (!protectedIds.has(id)) summaries.delete(id); }
  } catch (error) {
    if (current === request) { ids.forEach(id => failedSummaries.add(id)); notify(error); }
  } finally {
    summaryBusy = false;
    render();
  }
}
async function next() {
  if (paging || complete) return;
  paging = true;
  const current = request;
  try {
    const before = performance.now();
    const data = await invoke<Page>("next_page", { request: current, offset: hits.length });
    if (current !== request || data.request !== current) return;
    if (!painted) {
      timings.engineMs += data.engine_ms; timings.readMs += data.read_ms; timings.verifyMs += data.verify_ms;
      timings.searchRoundTripMs += performance.now() - before;
    }
    hits.push(...data.hits); complete = data.complete; updateVirtual(); revealFirstHit();
  } catch (error) { if (current === request) { complete = true; notify(error); } }
  finally { if (current === request) paging = false; render(); }
}
async function select(index: number) {
  const hit = hits[index]; if (!hit) return;
  selected = index; render();
  el("preview").hidden = false; el("preview-empty").hidden = true;
  el("file-name").textContent = hit.name; el("file-path").textContent = hit.path;
  el("snippets").textContent = "正在读取命中片段…";
  const current = request;
  try {
    const [preview] = await invoke<Preview[]>("previews", { request: current, ids: [hit.id], detail: true });
    if (request !== current || hits[selected]?.id !== hit.id || preview.request !== current) return;
    el("snippets").replaceChildren(...preview.snippets.map(snippet => {
      const section = document.createElement("section"); section.className = "snippet";
      const heading = document.createElement("h3");
      heading.textContent = snippet.location ? "第 " + snippet.location + ({ line: " 行", paragraph: " 段", page: " 页" }[snippet.kind]) : "位置不可用";
      const text = document.createElement("p"); textParts(text, snippet.parts); section.append(heading, text); return section;
    }));
  } catch (error) { if (current === request && hits[selected]?.id === hit.id) el("snippets").textContent = String(error); }
}
function beginSearch() {
  if (composing || !ready) return;
  request += 1; started = performance.now(); painted = false;
  timings = { engineMs: 0, readMs: 0, verifyMs: 0, searchRoundTripMs: 0, summaryRoundTripMs: 0 };
  hits = []; complete = false; selected = -1; activeKey = ""; collapsed.clear(); paging = true;
  summaries.clear(); failedSummaries.clear(); scroller.scrollTop = 0;
  el("preview").hidden = true; el("preview-empty").hidden = false;
  message.textContent = query.value ? "正在查找…" : "输入连续原文，查找包含它的文件。";
  queuedQuery = { request, query: query.value, extensions: resultType.value ? [resultType.value] : null };
  updateVirtual();
  void dispatchSearch();
}
async function dispatchSearch() {
  if (querying || !queuedQuery) return;
  querying = true;
  const task = queuedQuery; queuedQuery = null;
  try {
    const before = performance.now();
    const data = await invoke<Page>("search", task);
    if (task.request !== request) return;
    timings.engineMs = data.engine_ms; timings.readMs = data.read_ms; timings.verifyMs = data.verify_ms; timings.searchRoundTripMs = performance.now() - before;
    hits = data.hits; complete = data.complete; paging = false; updateVirtual(); revealFirstHit();
    if (!query.value) message.textContent = "输入连续原文，查找包含它的文件。";
  } catch (error) {
    if (task.request === request) { complete = true; paging = false; notify(error); message.textContent = String(error); }
  } finally {
    querying = false;
    if (queuedQuery) void dispatchSearch();
  }
}
query.addEventListener("compositionstart", () => { composing = true; });
query.addEventListener("compositionend", () => { composing = false; compositionValue = query.value; beginSearch(); });
query.addEventListener("input", event => {
  if ((event as InputEvent).isComposing) return;
  const duplicateCommit = compositionValue === query.value;
  compositionValue = null;
  if (!duplicateCommit) beginSearch();
});
scroller.addEventListener("keydown", event => {
  let target = rows.findIndex(row => row.key === activeKey);
  const entry = rows[target];
  const step = Math.max(1, Math.floor(scroller.clientHeight / 94));
  if (event.key === "ArrowDown") target++;
  else if (event.key === "ArrowUp") target--;
  else if (event.key === "Home") target = 0;
  else if (event.key === "End") { target = rows.length - 1; if (!complete) void next(); }
  else if (event.key === "PageDown") target += step;
  else if (event.key === "PageUp") target -= step;
  else if (event.key === "ArrowRight" && entry?.hit === null) {
    if (collapsed.has(entry.key)) { event.preventDefault(); toggleFolder(entry); return; }
    target++;
  } else if (event.key === "ArrowLeft" && entry) {
    if (entry.hit === null && !collapsed.has(entry.key)) { event.preventDefault(); toggleFolder(entry); return; }
    target = rows.findIndex(row => row.key === entry.parent);
  } else if ((event.key === "Enter" || event.key === " ") && entry) {
    event.preventDefault();
    if (entry.hit === null) toggleFolder(entry);
    else if (event.key === "Enter") void open(false);
    return;
  } else return;
  event.preventDefault();
  const targetRow = rows[Math.max(0, Math.min(rows.length - 1, target))];
  if (!targetRow) return;
  activeKey = targetRow.key;
  virtual.scrollToIndex(rows.indexOf(targetRow), { align: "auto" });
  if (targetRow.hit !== null) void select(targetRow.hit); else render();
});
document.addEventListener("keydown", event => { if (event.ctrlKey && event.key.toLowerCase() === "k") { event.preventDefault(); query.focus(); query.select(); } });
button("more").addEventListener("click", () => void next());
let statisticsBusy = false;
button("details").addEventListener("click", async () => {
  const hidden = !el("index-details").hidden;
  el("index-details").hidden = hidden; button("details").setAttribute("aria-expanded", String(!hidden));
  if (hidden || !ready || statisticsBusy) return;
  statisticsBusy = true; el("inventory").textContent = "正在汇总已提交的文件…";
  try {
    const stats = await invoke<Statistics>("index_statistics");
    el("inventory").textContent = "本次打开时的索引快照：" + stats.total.toLocaleString() + " 个文件，源文件合计 " + (stats.source_bytes / 1024 ** 3).toFixed(3) + " GiB；文本 " + stats.text.toLocaleString() + "，DOCX " + stats.docx.toLocaleString() + "，PDF " + stats.pdf.toLocaleString() + "。";
  } catch (error) { el("inventory").textContent = String(error); }
  finally { statisticsBusy = false; }
});
function taskButtons() {
  button("refresh").disabled = engineBusy || submittingRefresh || savingTypes;
  button("rebuild").disabled = engineBusy || submittingRefresh || savingTypes || !ready;
  button("types").disabled = savingTypes || submittingRefresh;
  button("types").textContent = savingTypes ? "正在应用类型…" : "索引类型";
  button("cancel").disabled = !canCancel;
}
async function refresh(rebuild: boolean) {
  if (submittingRefresh || savingTypes) return;
  submittingRefresh = true; taskButtons();
  const feedback = rebuild ? "正在提交重建任务，可继续搜索已有索引…" : "正在提交索引刷新任务…";
  notify(feedback); el("index-status").textContent = feedback;
  try {
    await invoke("refresh_index", { rebuild });
    await poll();
  } catch (error) { notify(error); }
  finally { submittingRefresh = false; taskButtons(); }
}
const rebuildDialog = el<HTMLDialogElement>("rebuild-dialog");
button("refresh").addEventListener("click", () => void refresh(false));
button("rebuild").addEventListener("click", () => rebuildDialog.showModal());
button("rebuild-close").addEventListener("click", () => rebuildDialog.close());
button("rebuild-confirm").addEventListener("click", () => { rebuildDialog.close(); void refresh(true); });
button("cancel").addEventListener("click", () => { void invoke("cancel_index").then(() => notify("正在取消，当前同步文本任务会在文件边界停止…")).catch(notify); });
async function open(folder: boolean) {
  if (selected < 0) return;
  try { await invoke("open_selected", { request, id: hits[selected].id, folder }); } catch (error) { notify(error); }
}
button("open").addEventListener("click", () => void open(false));
button("folder").addEventListener("click", () => void open(true));
button("copy").addEventListener("click", async () => {
  if (selected < 0) return;
  const current = request; const id = hits[selected].id;
  try {
    const path = await invoke<string>("selected_path", { request: current, id });
    if (request !== current || hits[selected]?.id !== id) return;
    await navigator.clipboard.writeText(path); notify("文件路径已复制");
  } catch (error) { notify(error); }
});
let polling = false;
async function poll() {
  if (polling) return;
  polling = true;
  try {
    const status = await invoke<Status>("engine_status");
    availableTypes = status.available_extensions;
    defaultTypes = status.default_extensions;
    const typesChanged = syncTypes(status.extensions);
    const busy = ["queued", "starting", "scanning", "rebuilding"].includes(status.phase);
    ready = status.ready; query.disabled = !ready;
    engineBusy = busy; canCancel = busy && status.phase !== "queued"; taskButtons();
    button("refresh").textContent = ready ? "刷新索引" : "建立本机索引";
    el("index-status").textContent = status.message + " · 已索引 " + status.indexed.toLocaleString() + " 个文件" + (busy ? " · 已检查 " + status.checked.toLocaleString() : "") + (status.failed ? " · 累计 " + status.failed + " 次失败或跳过" : "") + (!status.complete && ready ? " · 覆盖尚未完整" : "");
    el("roots").textContent = "扫描范围：" + status.roots.join("；");
    el("exclusions").textContent = "排除：" + status.exclusions.join("；");
    el("coverage").textContent = "最近提交：" + (status.last_updated ? new Date(status.last_updated * 1000).toLocaleString() : "尚无") + " · " + (status.complete ? "范围复核完成" : "范围尚未完整复核");
    el("failures").replaceChildren(...status.failures.map(failure => { const li = document.createElement("li"); li.textContent = failure.reason + " — " + failure.path; return li; }));
    if (typesChanged || (version !== status.version && !busy)) {
      version = status.version;
      if (query.value) { beginSearch(); notify("索引已更新，搜索结果已刷新。"); }
    }
    if (ready && !query.value && !started) message.textContent = "输入连续原文，查找包含它的文件。";
  } catch { el("index-status").textContent = "无法连接本机引擎，请通过桌面程序启动。"; }
  finally { polling = false; }
}
query.addEventListener("keydown", event => { if (event.key === "Enter" && !composing) beginSearch(); });
void poll(); setInterval(() => void poll(), 1000);

function syncTypes(types: string[]) {
  if (types.join(",") === configuredTypes.join(",")) return false;
  configuredTypes = types;
  const previous = resultType.value;
  resultType.replaceChildren(new Option("全部已选类型", ""), ...types.map(type => new Option(type.toUpperCase(), type)));
  resultType.value = types.includes(previous) ? previous : "";
  return true;
}
resultType.addEventListener("change", beginSearch);
button("expand-all").addEventListener("click", () => { collapsed.clear(); updateVirtual(); });
button("collapse-all").addEventListener("click", () => {
  treeRows(hits, new Set()).filter(row => row.hit === null).forEach(row => collapsed.add(row.key));
  activeKey = ""; updateVirtual(); scroller.scrollTop = 0;
});
const typesDialog = el<HTMLDialogElement>("types-dialog");
let savingTypes = false;
function chosenTypes() { return [...typesDialog.querySelectorAll<HTMLInputElement>("input:checked")].map(input => input.value); }
function updateTypeSave() { button("types-save").disabled = savingTypes || chosenTypes().length === 0; }
button("types").addEventListener("click", () => {
  const fieldset = el("type-options");
  fieldset.querySelectorAll("label").forEach(label => label.remove());
  fieldset.append(...availableTypes.map(type => {
    const label = document.createElement("label");
    const input = document.createElement("input"); input.type = "checkbox"; input.value = type; input.checked = configuredTypes.includes(type);
    input.addEventListener("change", updateTypeSave);
    label.append(input, document.createTextNode(type.toUpperCase())); return label;
  }));
  el("types-notice").textContent = "当前索引任务将在文件边界停止，然后按新选择继续。";
  updateTypeSave(); typesDialog.showModal();
});
for (const id of ["types-default", "types-all"]) button(id).addEventListener("click", () => {
  typesDialog.querySelectorAll<HTMLInputElement>("input").forEach(input => { input.checked = id === "types-all" || defaultTypes.includes(input.value); }); updateTypeSave();
});
button("types-close").addEventListener("click", () => typesDialog.close());
button("types-save").addEventListener("click", async () => {
  const extensions = chosenTypes(); if (!extensions.length || savingTypes) return;
  savingTypes = true; taskButtons(); typesDialog.close();
  const feedback = "正在后台应用类型；等待当前文件结束后清理旧索引，可继续使用页面…";
  notify(feedback); el("index-status").textContent = feedback;
  try {
    await invoke("set_index_types", { extensions });
    syncTypes(extensions); await poll(); beginSearch();
    notify("索引类型已保存，正在按新选择更新索引。");
  } catch (error) { notify("应用类型失败：" + String(error)); }
  finally { savingTypes = false; taskButtons(); updateTypeSave(); }
});
const recycleDialog = el<HTMLDialogElement>("recycle-dialog");
let pendingDelete: { request: number; id: number } | null = null;
let deleting = false;
button("delete").addEventListener("click", () => {
  const hit = hits[selected]; if (!hit) return;
  pendingDelete = { request, id: hit.id };
  el("recycle-path").textContent = hit.path;
  el("recycle-notice").textContent = "";
  recycleDialog.showModal();
});
button("recycle-close").addEventListener("click", () => recycleDialog.close());
recycleDialog.addEventListener("cancel", event => { if (deleting) event.preventDefault(); });
button("recycle-confirm").addEventListener("click", async () => {
  if (!pendingDelete || deleting) return;
  deleting = true; button("recycle-confirm").disabled = true; button("recycle-close").disabled = true;
  el("recycle-notice").textContent = "正在移入回收站并更新索引…";
  try {
    await invoke("recycle_selected", pendingDelete);
    recycleDialog.close(); beginSearch(); notify("文件已移入 Windows 回收站，索引已更新。");
  } catch (error) { el("recycle-notice").textContent = String(error); }
  finally { deleting = false; button("recycle-confirm").disabled = false; button("recycle-close").disabled = false; }
});
