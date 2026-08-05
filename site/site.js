const menuButton = document.querySelector(".hamburger");
const mobilePanel = document.querySelector(".mobile-panel");

menuButton?.addEventListener("click", () => {
  const open = menuButton.getAttribute("aria-expanded") !== "true";
  menuButton.setAttribute("aria-expanded", String(open));
  menuButton.setAttribute("aria-label", open ? "Close menu" : "Menu");
  mobilePanel?.setAttribute("data-open", String(open));
});

mobilePanel?.addEventListener("click", (event) => {
  if (!(event.target instanceof HTMLAnchorElement)) return;
  menuButton?.setAttribute("aria-expanded", "false");
  menuButton?.setAttribute("aria-label", "Menu");
  mobilePanel.setAttribute("data-open", "false");
});

document.querySelectorAll("[data-copy]").forEach((copyButton) => copyButton.addEventListener("click", async (event) => {
  const button = event.currentTarget;
  if (!(button instanceof HTMLButtonElement)) return;
  const lines = button.closest(".install-row")?.querySelectorAll("[data-install-command] > div");
  const command = lines ? [...lines]
    .map((line) => line.textContent?.trim().replace(/^\$\s*/, ""))
    .filter(Boolean)
    .join("\n") : "";
  if (!command) return;
  try {
    await navigator.clipboard.writeText(command);
    button.textContent = "copied";
    window.setTimeout(() => { button.textContent = "copy"; }, 1600);
  } catch {
    button.textContent = "select command";
  }
}));

try {
  const statsElement = document.querySelector("#repo-stats");
  const stats = JSON.parse(statsElement?.textContent || "{}");
  const starCount = document.querySelector('[data-stat="stars"]');
  if (starCount && stats.stars) {
    starCount.textContent = stats.stars;
    starCount.hidden = false;
  }

  let visibleActivity = 0;
  for (const key of ["stars", "contributors", "commits", "lastCommit"]) {
    if (!stats[key]) continue;
    const cell = document.querySelector(`[data-activity-stat="${key}"]`);
    const value = cell?.querySelector("strong");
    if (!cell || !value) continue;
    value.textContent = stats[key];
    cell.hidden = false;
    visibleActivity += 1;
  }
  const activity = document.querySelector("[data-activity]");
  if (activity && visibleActivity > 0) activity.hidden = false;
} catch {
  // Build-time GitHub data is optional; omit the activity figures when unavailable.
}
