(async () => {
  const $ = id => document.getElementById(id);
  const assert = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (predicate, timeout = 10000) => {
    const start = performance.now();
    while (!predicate()) {
      if (performance.now() - start > timeout) throw new Error("Management UI timed out");
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  assert($("roots").textContent.includes("FileContentSearch-demo-"), "Only isolated synthetic demo files may be changed");
  const search = async text => {
    $("query").value = text; $("query").dispatchEvent(new InputEvent("input", { bubbles: true }));
    await wait(() => $("more").hidden && !$("count").textContent.includes("+"));
  };
  const filter = async value => {
    $("result-type").value = value; $("result-type").dispatchEvent(new Event("change"));
    await wait(() => $("more").hidden);
  };
  await wait(() => !$("refresh").disabled);
  $("query").value = "";
  await filter("");
  await search("管理专项样本");
  await wait(() => $("count").textContent === "3 个文件");
  assert($("count").textContent === "3 个文件", "Three nested synthetic matches expected");
  await filter("md");
  assert($("count").textContent === "1 个文件", "Type filtering must query the backend");
  assert([...document.querySelectorAll(".result .type")].every(node => node.textContent === "MD"), "Only Markdown should match");
  await filter("");
  $("collapse-all").click();
  assert(document.querySelectorAll(".result").length === 0 && document.querySelectorAll(".tree-folder").length > 0, "Collapse hides descendants");
  $("expand-all").click();
  await wait(() => document.querySelectorAll(".result").length > 0);
  $("types").click();
  assert($("types-dialog").open, "Index type dialog opens");
  assert(!$("type-options").querySelector('input[value="json"]'), "JSON stays excluded");
  $("type-options").querySelectorAll("input").forEach(input => { input.checked = ["txt", "docx", "pdf"].includes(input.value); });
  $("type-options").querySelector("input").dispatchEvent(new Event("change")); $("types-save").click();
  await wait(() => !$("types-dialog").open && !$("refresh").disabled);
  await search("管理专项样本");
  assert($("count").textContent === "2 个文件", "Unchecked type removed from old index");
  assert(![...$("result-type").options].some(option => option.value === "md"), "Result filter follows indexed types");
  $("results").scrollTop = $("results").scrollHeight;
  await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const target = [...document.querySelectorAll(".result")].find(node => node.textContent.includes("a1.txt"));
  assert(target, "Nested target remains visible"); target.click();
  await wait(() => $("file-name").textContent === "a1.txt" && $("snippets").querySelector("mark"));
  $("delete").click();
  assert($("recycle-dialog").open && $("recycle-path").textContent.endsWith("a1.txt"), "Deletion confirmation identifies one file");
  $("recycle-close").click();
  assert($("count").textContent === "2 个文件", "Cancel preserves result");
  $("delete").click(); $("recycle-confirm").click();
  await wait(() => !$("recycle-dialog").open);
  await wait(() => $("count").textContent === "1 个文件" && $("more").hidden);
  assert(![...document.querySelectorAll(".result")].some(node => node.textContent.includes("a1.txt")), "Recycled file disappears from search");
  $("types").click(); $("types-all").click(); $("types-save").click();
  await wait(() => !$("types-dialog").open && !$("refresh").disabled);
  await search("管理专项样本");
  await wait(() => $("count").textContent === "2 个文件");
  return { typeFilter: "passed", typeSelectionReconciliation: "passed", folderCollapseExpand: "passed", recycleCancelAndConfirm: "passed", reenableType: "passed" };
})();
