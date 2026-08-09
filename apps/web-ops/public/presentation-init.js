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
})();
