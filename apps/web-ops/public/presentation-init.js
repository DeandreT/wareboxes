(() => {
  const root = document.documentElement;
  const read = (key, fallback) => {
    try {
      return window.localStorage.getItem(key) || fallback;
    } catch {
      return fallback;
    }
  };

  root.dataset.theme = read("wareboxes.display.theme", "system");
  root.dataset.density = read("wareboxes.display.density", "compact");
  root.dataset.reduceMotion = read(
    "wareboxes.display.reduce-motion",
    "false",
  );
  root.dataset.navigationHidden = read(
    "wareboxes.display.hide-navigation",
    "false",
  );

  const openDisplayOptions = () =>
    document.querySelectorAll("details.display-options[open]");

  document.addEventListener("pointerdown", (event) => {
    for (const menu of openDisplayOptions()) {
      if (event.target instanceof Node && menu.contains(event.target)) {
        continue;
      }
      menu.open = false;
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") {
      return;
    }

    const menu = openDisplayOptions().item(0);
    if (!menu) {
      return;
    }

    menu.open = false;
    menu.querySelector("summary")?.focus();
  });
})();
