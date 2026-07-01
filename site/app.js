/* Perenne — animated boot console.
   Replays a curated slice of the real serial output. Faults are amber,
   healing is jade — the palette tells the self-healing story. */
(function () {
  "use strict";
  var body = document.getElementById("term-body");
  var replayBtn = document.getElementById("replay");
  if (!body) return;

  // Curated from real ./tools/test-qemu.ps1 output (two boots, one disk image).
  var LINES = [
    { c: "cmd",   s: "./tools/run-qemu.ps1" },
    { c: "",      s: "hello world from Perenne — Phase 4a (hart 0)" },
    { c: "dim",   s: "paging: sv39 on · W^X enforced · frames ok" },
    { c: "heal",  s: "crypto: channel session established (ML-KEM)" },
    { c: "",      s: "net: dhcp leased 10.0.2.15 (ack)" },
    { c: "",      s: "net: adopted ip 10.0.2.15" },
    { c: "",      s: "net: resolved 10.0.2.2 → 52:55:0a:00:02:02" },
    { c: "",      s: "net: ping 10.0.2.2: reply (seq 0)" },
    { c: "",      s: "net: dns example.com → 104.20.23.154" },
    { c: "dim",   s: "shell: ready (type 'help')" },
    { c: "fault", s: "sched: task 'transient' killed by LoadPageFault" },
    { c: "",      s: "heal: diagnosed KB-0005 → restart the component" },
    { c: "heal",  s: "heal: restarted 'transient' — recovered ✓" },
    { c: "fault", s: "sched: task 'novel' killed by IllegalInstruction" },
    { c: "",      s: "heal: no known issue — recording for write-back" },
    { c: "heal",  s: "heal: recorded KB-0006 (illegal-instruction) to disk" },
    { c: "dim",   s: "— reboot · same disk image —" },
    { c: "",      s: "heal: loaded 2 KB entries from disk" },
    { c: "heal",  s: "heal: diagnosed KB-0006 — the fault it wrote itself" },
    { c: "fault", s: "sched: task 'flaky' killed (again)" },
    { c: "fault", s: "heal: KB-0005 escalated (seen 6) — chronic" },
    { c: "heal",  s: "heal: 'flaky' quarantined — not restarting" },
    { c: "final", s: "✦ recovered · remembered · learned" }
  ];

  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var timer = null;

  function mkLine(cls) {
    var p = document.createElement("p");
    p.className = "term-line" + (cls ? " " + cls : "");
    body.appendChild(p);
    return p;
  }
  function mkCursor() {
    var s = document.createElement("span");
    s.className = "term-cursor";
    return s;
  }
  function clearAll() {
    if (timer) { clearTimeout(timer); timer = null; }
    var nodes = body.querySelectorAll(".term-line");
    for (var i = 0; i < nodes.length; i++) nodes[i].remove();
  }

  function renderInstant() {
    clearAll();
    for (var i = 0; i < LINES.length; i++) {
      var el = mkLine(LINES[i].c);
      el.appendChild(document.createTextNode(LINES[i].s));
    }
    var prompt = mkLine("cmd");
    prompt.appendChild(mkCursor());
  }

  function typeLines(i) {
    if (i >= LINES.length) {
      var prompt = mkLine("cmd");
      prompt.appendChild(mkCursor());
      return;
    }
    var ln = LINES[i];
    var el = mkLine(ln.c);
    var textNode = document.createTextNode("");
    el.appendChild(textNode);
    var cur = mkCursor();
    el.appendChild(cur);
    var speed = ln.c === "cmd" ? 32 : 8;
    var j = 0;
    (function typeChar() {
      if (j < ln.s.length) {
        textNode.data += ln.s.charAt(j++);
        timer = setTimeout(typeChar, speed);
      } else {
        cur.remove();
        var pause = ln.c === "dim" ? 300 : (ln.c === "cmd" ? 260 : 120);
        timer = setTimeout(function () { typeLines(i + 1); }, pause);
      }
    })();
  }

  function play() {
    clearAll();
    if (reduce) { renderInstant(); return; }
    timer = setTimeout(function () { typeLines(0); }, 350);
  }

  if (replayBtn) {
    replayBtn.addEventListener("click", function () {
      play();
      document.querySelector(".term").scrollIntoView({ block: "nearest", behavior: reduce ? "auto" : "smooth" });
    });
  }

  play();
})();
