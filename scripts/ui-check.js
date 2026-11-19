(async () => {
  const $ = id => document.getElementById(id);
  const assert = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (predicate, timeout = 5000) => {
    const start = performance.now();
    while (!predicate()) {
      if (performance.now() - start > timeout) throw new Error("UI condition timed out");
      await new Promise(resolve => setTimeout(resolve, 10));
    }
  };
  assert($("roots").textContent.includes("FileContentSearch-demo-"), "UI checks require isolated --demo mode");
  await wait(() => !$("refresh").disabled && $("index-status").textContent.includes("10,003"), 30000);
  const components = [];
  const search = text => new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { window.removeEventListener("search-painted", listener); reject(new Error("Search paint timed out: " + text)); }, 5000);
    const listener = event => { clearTimeout(timeout); window.removeEventListener("search-painted", listener); components.push(event.detail); resolve(event.detail.milliseconds); };
    window.addEventListener("search-painted", listener);
    $("query").value = text;
    $("query").dispatchEvent(new InputEvent("input", { bubbles: true }));
  });
  // Fixed before performance measurement; never remove slower queries after measuring.
  const ordinaryQueries = ["合同", "合同签署", "研发计划", "项目验收", "会议记录", "采购申请", "测试报告", "知识管理", "财务报表", "不存在的词"];
  const coldFirst = await search(ordinaryQueries[0]);
  const ordinary = [];
  for (let repetition = 0; repetition < 5; repetition++) for (const text of ordinaryQueries) ordinary.push(await search(text));
  const frequent = [];
  for (const text of ["合", "内容搜索", "AND", "%", "_", "😀𠮷", "哈哈哈"]) frequent.push({ query: text, milliseconds: await search(text) });
  const samples = values => { const sorted = [...values].sort((a, b) => a - b); return { count: sorted.length, p50: sorted[Math.ceil(sorted.length * .5) - 1], p95: sorted[Math.ceil(sorted.length * .95) - 1], max: sorted.at(-1) }; };
  await search("内容搜索");
  let pages = 0;
  while (!$("more").hidden) {
    await wait(() => !$("more").disabled);
    const before = $("count").textContent;
    $("more").click();
    await wait(() => $("count").textContent !== before || $("more").hidden);
    assert(++pages <= 205, "Pagination did not terminate");
  }
  assert($("count").textContent === "10,000 个文件", "Ten thousand exact results expected");
  const scrollTimes = [];
  let maximumRows = 0;
  for (let fraction = 0; fraction <= 1; fraction += .05) {
    const before = performance.now();
    $("results").scrollTop = fraction * $("results").scrollHeight;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    scrollTimes.push(performance.now() - before);
    const rows = [...document.querySelectorAll(".result")]; maximumRows = Math.max(maximumRows, rows.length);
    assert(rows.length < 24, "Virtual list rendered too many rows");
    const row = rows.find(row => row.getBoundingClientRect().top >= $("results").getBoundingClientRect().top);
    row?.click();
    if (row) await wait(() => $("file-name").textContent === row.querySelector(".result-title span:last-child").textContent && $("snippets").querySelector("mark"));
  }
  const frames = [];
  const longTasks = [];
  const observer = new PerformanceObserver(list => { for (const entry of list.getEntries()) longTasks.push(entry.duration); });
  observer.observe({ type: "longtask", buffered: false });
  let previous = await new Promise(requestAnimationFrame);
  for (let frame = 0; frame < 120; frame++) {
    $("results").scrollTop = (frame / 119) * $("results").scrollHeight;
    const time = await new Promise(requestAnimationFrame);
    frames.push(time - previous); previous = time;
  }
  observer.disconnect();
  $("results").focus();
  $("results").dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
  await wait(() => document.querySelector('.tree-folder[aria-selected="true"]')?.getAttribute("aria-level") === "1");
  for (let depth = 0; depth < 20 && !document.querySelector('.result[aria-selected="true"]'); depth++) {
    $("results").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await new Promise(resolve => requestAnimationFrame(resolve));
  }
  await wait(() => document.querySelector('.result[aria-selected="true"]')?.getAttribute("aria-posinset") === "1");
  $("results").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  await wait(() => document.querySelector('.result[aria-selected="true"]')?.getAttribute("aria-posinset") === "2");
  await search("项目验收");
  const beforeComposition = components.at(-1).request;
  const ime = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("IME search timed out")), 5000);
    window.addEventListener("search-painted", event => { clearTimeout(timeout); resolve(event.detail.request); }, { once: true });
  });
  $("query").dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  $("query").value = "研发计划";
  $("query").dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: true }));
  $("query").dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "研发计划" }));
  $("query").dispatchEvent(new InputEvent("input", { bubbles: true, isComposing: false }));
  assert(await ime === beforeComposition + 1, "IME commit must submit one query");
  for (const text of ["合同", "项目", "不存在的词", "测试报告"]) { $("query").value = text; $("query").dispatchEvent(new InputEvent("input", { bubbles: true })); }
  await wait(() => [...document.querySelectorAll(".result-summary mark")].length > 0 && [...document.querySelectorAll(".result-summary mark")].every(mark => mark.textContent === "测试报告"));
  const breakdown = {};
  for (const key of ["engineMs", "readMs", "verifyMs", "searchRoundTripMs", "summaryRoundTripMs"]) breakdown[key] = samples(components.slice(1, 51).map(sample => sample[key]));
  const result = { viewport: [innerWidth, innerHeight], coldFirst: { note: "first query in this run, OS cache not flushed", milliseconds: coldFirst }, ordinaryQueries, ordinary: samples(ordinary), breakdown, frequent, loadedResults: 10000, maximumRows, scroll: samples(scrollTimes), frameIntervals: samples(frames), longTasks, keyboardImeAndRace: "passed" };
  window.__fcsUiCheck = result;
  return result;
})();
