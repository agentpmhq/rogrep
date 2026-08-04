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

document.querySelector("[data-copy]")?.addEventListener("click", async (event) => {
  const button = event.currentTarget;
  if (!(button instanceof HTMLButtonElement)) return;
  const command = document.querySelector(".install-command")?.textContent?.trim();
  if (!command) return;
  try {
    await navigator.clipboard.writeText(command);
    button.textContent = "copied";
    window.setTimeout(() => { button.textContent = "copy"; }, 1600);
  } catch {
    button.textContent = "select command";
  }
});
