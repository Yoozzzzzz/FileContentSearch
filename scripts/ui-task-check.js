(async () => {
  const $ = id => document.getElementById(id);
  const assert = (ok, message) => { if (!ok) throw new Error(message); };
  const wait = async (predicate, timeout = 15000) => {
    const start = performance.now();
    while (!predicate()) {
      if (performance.now() - start > timeout) throw new Error("Task UI timed out");
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  assert($("roots").textContent.includes("FileContentSearch-demo-"), "Task tests require isolated synthetic demo data");
  await wait(() => !$("refresh").disabled && $("index-status").textContent.includes("10,003"));
  const fetch = window.fetch.bind(window);
  const nativeConfirm = window.confirm;
  let held = null;
  let calls = 0;
  const phases = [];
  window.fetch = (resource, options) => {
    if (held && String(resource).endsWith("/" + held.command)) {
      calls++;
      return new Promise((resolve, reject) => {
        held.release = () => { held = null; fetch(resource, options).then(resolve, reject); };
        held.fail = () => {
          held = null;
          resolve(new Response(JSON.stringify("合成提交失败"), { headers: { "Tauri-Response": "error", "Content-Type": "application/json" } }));
        };
      });
    }
    return fetch(resource, options);
  };
  window.confirm = () => { throw new Error("Blocking native confirmation must not be used"); };
  async function responsive(label) {
    assert(!document.querySelector("dialog[open]"), label + ": modal must close before backend completion");
    $("query").focus();
    $("query").value = "合同";
    $("query").dispatchEvent(new InputEvent("input", { bubbles: true }));
    const started = performance.now();
    let frames = 0;
    while (performance.now() - started < 1200) {
      await new Promise(requestAnimationFrame); frames++;
    }
    assert(frames >= 5, label + ": rendering stalled");
    assert(document.activeElement === $("query"), label + ": search input is blocked");
    assert($("query").value === "合同" && !$("query").disabled, label + ": input must remain usable");
    assert($("refresh").disabled && $("rebuild").disabled, label + ": duplicate submission must stay disabled during polling");
    phases.push({ phase: label, frames, observedMs: performance.now() - started });
  }
  async function finishWork(label, predicate) {
    let done = false, frames = 0, maxFrameGap = 0, previous = performance.now();
    const animation = (async () => {
      while (!done) {
        const now = await new Promise(requestAnimationFrame);
        maxFrameGap = Math.max(maxFrameGap, now - previous); previous = now; frames++;
      }
    })();
    try { await wait(predicate); } finally { done = true; await animation; }
    assert(maxFrameGap < 500, label + ": page froze while the real backend was working");
    phases.push({ phase: label, frames, maxFrameGap });
  }
  try {
    held = { command: "refresh_index" };
    $("rebuild").click();
    assert($("rebuild-dialog").open && calls === 0, "Rebuild must first show a page confirmation");
    $("rebuild-close").click();
    assert(calls === 0, "Cancel must not submit rebuild");
    $("rebuild").click(); $("rebuild-confirm").click();
    await wait(() => held?.release);
    await responsive("rebuild submission pending");
    held.release();
    await finishWork("real rebuild", () => !$("refresh").disabled);
    assert($("index-status").textContent.includes("10,003"), "Rebuild must retain the corpus");

    held = { command: "set_index_types" };
    $("types").click(); $("types-default").click();
    const selected = [...$("type-options").querySelectorAll("input:checked")].map(input => input.value).sort();
    assert(JSON.stringify(selected) === JSON.stringify(["docx", "markdown", "md", "pdf"]), "Default selection must contain exactly the four requested types");
    $("types-save").click();
    await wait(() => held?.release);
    await responsive("type application pending");
    held.release();
    await finishWork("real type cleanup", () => !$("types").disabled && !$("refresh").disabled);
    assert($("index-status").textContent.includes("已索引 2 个文件"), "Applying defaults must remove the demo TXT index entries");
    assert(![...$("result-type").options].some(option => option.value === "txt"), "Filter must follow the applied types");

    held = { command: "set_index_types" };
    $("types").click(); $("types-all").click(); $("types-save").click();
    await wait(() => held?.fail); held.fail();
    await wait(() => !$("types").disabled);
    assert($("notice").textContent.includes("合成提交失败"), "Failure must stay visible outside the closed dialog");
    assert(![...$("result-type").options].some(option => option.value === "txt"), "Failed submission must not claim the selection was saved");

    window.fetch = fetch;
    $("types").click(); $("types-all").click(); $("types-save").click();
    await wait(() => !$("types").disabled && !$("refresh").disabled && $("index-status").textContent.includes("10,003"));
    return { phases, defaults: selected, rebuildCancelAndCompletion: "passed", typeApplyAndRetry: "passed", failureFeedback: "passed" };
  } finally {
    window.fetch = fetch;
    window.confirm = nativeConfirm;
  }
})();
