(async () => {
  if (!document.getElementById("roots").textContent.includes("FileContentSearch-demo-")) throw new Error("Synthetic demo required");
  const measurements = [];
  const breakdown = [];
  for (let i = 0; i < 20; i++) {
    measurements.push(await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("Large-document query timed out")), 10000);
      window.addEventListener("search-painted", event => { clearTimeout(timeout); breakdown.push(event.detail); resolve(event.detail.milliseconds); }, { once: true });
      const input = document.getElementById("query"); input.value = "大文件合同验收"; input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    }));
    if (document.getElementById("count").textContent !== "1 个文件") throw new Error("Large sample not indexed");
  }
  const sorted = [...measurements].sort((a, b) => a - b);
  return { query: "大文件合同验收", repetitions: 20, p50: sorted[9], p95: sorted[18], max: sorted[19], breakdown };
})();
