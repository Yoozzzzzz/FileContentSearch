(async () => {
  const input = document.getElementById("query");
  if (!input || input.disabled) throw new Error("Open the desktop app with an available index first");
  if (!document.getElementById("cancel").disabled) throw new Error("Wait for indexing to finish or pause it before measuring");
  const samples = values => {
    const sorted = [...values].sort((a, b) => a - b);
    return { count: sorted.length, p50: sorted[Math.ceil(sorted.length * .5) - 1], p95: sorted[Math.ceil(sorted.length * .95) - 1], max: sorted.at(-1) };
  };
  const search = text => new Promise((resolve, reject) => {
    const timer = setTimeout(() => { window.removeEventListener("search-painted", painted); reject(new Error("Search did not finish within 30 seconds")); }, 30000);
    function painted(event) {
      clearTimeout(timer); window.removeEventListener("search-painted", painted);
      const marks = [...document.querySelectorAll(".result-summary mark")];
      const normalize = value => value.replace(/[A-Z]/g, character => character.toLowerCase());
      if (marks.some(mark => normalize(mark.textContent) !== normalize(text))) { reject(new Error("Highlight belongs to another query")); return; }
      resolve(event.detail);
    }
    window.addEventListener("search-painted", painted);
    input.value = text;
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
  });
  // Same fixed query set as ui-check.js; real-document contents and paths are never returned.
  const queries = ["合同", "合同签署", "研发计划", "项目验收", "会议记录", "采购申请", "测试报告", "知识管理", "财务报表", "不存在的词"];
  const first = await search(queries[0]);
  const ordinary = [];
  for (let repetition = 0; repetition < 5; repetition++) for (const query of queries) ordinary.push(await search(query));
  const frequent = [];
  for (const query of ["合", "的", "AND", "%", "_"]) frequent.push({ query, ...await search(query) });
  const breakdown = {};
  for (const key of ["engineMs", "readMs", "verifyMs", "searchRoundTripMs", "summaryRoundTripMs"]) breakdown[key] = samples(ordinary.map(sample => sample[key]));
  const result = {
    status: document.getElementById("index-status").textContent,
    first: { note: "First query after connecting; OS cache was not flushed", ...first },
    queries, ordinary: samples(ordinary.map(sample => sample.milliseconds)),
    byQuery: queries.map((query, index) => {
      const measured = ordinary.filter((_, position) => position % queries.length === index);
      return { query, visibleResults: measured[0].visible, ...samples(measured.map(sample => sample.milliseconds)) };
    }),
    breakdown, frequent,
  };
  input.value = ""; input.dispatchEvent(new InputEvent("input", { bubbles: true }));
  return result;
})();
