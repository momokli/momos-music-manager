/* mockup.js — Navigationsleiste zwischen den Mockups (nur Doku-Zweck) */
(function () {
  const MOCKUPS = [
    ["mockup-shell-dashboard.html", "M1 · Dashboard & Shell"],
    ["mockup-files-commentdiff.html", "M2 · Files & PMV-Review"],
    ["mockup-deemix-queue.html", "M3 · Deemix Queue"],
    ["mockup-digging.html", "M4 · Digging"],
    ["mockup-traktor-import.html", "M5 · Traktor Import"],
    ["mockup-settings-telemetry.html", "M6 · Settings & Telemetry"],
  ];
  const here = location.pathname.split("/").pop();
  const bar = document.createElement("div");
  bar.className = "mockup-bar";
  bar.innerHTML =
    '<b>UI/UX · Mockup</b><nav class="mnav">' +
    MOCKUPS.map(
      ([f, l]) => `<a href="${f}" class="${f === here ? "here" : ""}">${l}</a>`
    ).join("") +
    '</nav><span class="tag-note">Statische Vorschau — klickbare Fassade, keine echte App</span>';
  document.body.prepend(bar);
})();
