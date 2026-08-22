(() => {
  const root = document.documentElement;
  const storageKeys = {
    theme: "wareboxes.display.theme",
    density: "wareboxes.display.density",
    reduceMotion: "wareboxes.display.reduce-motion",
    navigationHidden: "wareboxes.display.hide-navigation",
  };

  const stored = (key) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  };
  const enumPreference = (key, allowed, fallback) => {
    const value = stored(key);
    return allowed.includes(value) ? value : fallback;
  };
  const booleanPreference = (key) =>
    stored(key) === "true" ? "true" : "false";

  const applyPreferences = () => {
    root.dataset.theme = enumPreference(
      storageKeys.theme,
      ["system", "light", "dark"],
      "system",
    );
    root.dataset.density = enumPreference(
      storageKeys.density,
      ["compact", "standard"],
      "compact",
    );
    root.dataset.reduceMotion = booleanPreference(storageKeys.reduceMotion);
    root.dataset.navigationHidden = booleanPreference(
      storageKeys.navigationHidden,
    );
  };

  const darkSystemTheme = window.matchMedia("(prefers-color-scheme: dark)");
  const updateThemeColor = () => {
    const dark =
      root.dataset.theme === "dark" ||
      (root.dataset.theme === "system" && darkSystemTheme.matches);
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", dark ? "#121617" : "#f4f6f5");
  };

  applyPreferences();
  updateThemeColor();
  darkSystemTheme.addEventListener?.("change", updateThemeColor);
  window.addEventListener("storage", (event) => {
    if (Object.values(storageKeys).includes(event.key)) {
      applyPreferences();
      updateThemeColor();
    }
  });
  new MutationObserver(updateThemeColor).observe(root, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });

  const startInteractions = () => {
    const openMenus = () =>
      document.querySelectorAll(
        "details.display-options[open], details.profile-menu[open]",
      );

    document.addEventListener("pointerdown", (event) => {
      for (const menu of openMenus()) {
        if (event.target instanceof Node && menu.contains(event.target)) {
          continue;
        }
        menu.open = false;
      }
    });

    let activeDialog = null;
    let returnFocus = null;
    const dialogSelector =
      '[role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"]';
    const focusableSelector = [
      "button:not([disabled])",
      "a[href]",
      "input:not([disabled]):not([type='hidden'])",
      "select:not([disabled])",
      "textarea:not([disabled])",
      "[tabindex]:not([tabindex='-1'])",
    ].join(",");

    const visibleFocusables = (dialog) =>
      [...dialog.querySelectorAll(focusableSelector)].filter(
        (element) =>
          element instanceof HTMLElement &&
          element.getClientRects().length > 0 &&
          element.getAttribute("aria-hidden") !== "true",
      );

    const topDialog = () => {
      const dialogs = [...document.querySelectorAll(dialogSelector)];
      return (
        dialogs
          .filter(
            (dialog) =>
              dialog instanceof HTMLElement &&
              dialog.getClientRects().length > 0,
          )
          .at(-1) || null
      );
    };

    const syncTabLists = () => {
      for (const tabList of document.querySelectorAll('[role="tablist"]')) {
        const tabs = [...tabList.querySelectorAll(':scope > [role="tab"]')];
        const selected =
          tabs.find((tab) => tab.getAttribute("aria-selected") === "true") ||
          tabs[0];
        for (const tab of tabs) {
          tab.tabIndex = tab === selected ? 0 : -1;
        }
      }
    };

    const syncDialog = () => {
      const nextDialog = topDialog();
      if (nextDialog === activeDialog) {
        return;
      }

      if (!nextDialog) {
        activeDialog = null;
        document.body.classList.remove("dialog-open");
        if (returnFocus instanceof HTMLElement && returnFocus.isConnected) {
          returnFocus.focus({ preventScroll: true });
        }
        returnFocus = null;
        return;
      }

      if (!activeDialog && document.activeElement instanceof HTMLElement) {
        returnFocus = document.activeElement;
      }
      activeDialog = nextDialog;
      document.body.classList.add("dialog-open");
      window.requestAnimationFrame(() => {
        if (
          !activeDialog ||
          (document.activeElement instanceof Node &&
            activeDialog.contains(document.activeElement))
        ) {
          return;
        }
        const initial =
          activeDialog.querySelector("[data-dialog-autofocus], [autofocus]") ||
          visibleFocusables(activeDialog)[0] ||
          activeDialog;
        if (initial instanceof HTMLElement) {
          if (initial === activeDialog && !initial.hasAttribute("tabindex")) {
            initial.setAttribute("tabindex", "-1");
          }
          initial.focus({ preventScroll: true });
        }
      });
    };

    const dialogObserver = new MutationObserver(() => {
      syncDialog();
      syncTabLists();
    });
    dialogObserver.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: [
        "class",
        "hidden",
        "open",
        "aria-hidden",
        "aria-selected",
      ],
    });
    syncDialog();
    syncTabLists();

    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        const menu = openMenus().item(0);
        if (menu) {
          menu.open = false;
          menu.querySelector("summary")?.focus();
          event.preventDefault();
          return;
        }

        if (!activeDialog) {
          return;
        }
        const explicitClose = activeDialog.querySelector(
          "[data-dialog-close], button[aria-label*='close' i], button[title*='close' i]",
        );
        const textualClose = [...activeDialog.querySelectorAll("button")].find(
          (button) => /^(cancel|close|done|keep editing)$/i.test(button.textContent.trim()),
        );
        (explicitClose || textualClose)?.click();
        if (explicitClose || textualClose) {
          event.preventDefault();
        }
        return;
      }

      if (event.key !== "Tab" || !activeDialog) {
        const tab =
          event.target instanceof Element
            ? event.target.closest('[role="tab"]')
            : null;
        const tabList = tab?.parentElement?.closest('[role="tablist"]');
        if (
          !tab ||
          !tabList ||
          !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
        ) {
          return;
        }
        const tabs = [...tabList.querySelectorAll(':scope > [role="tab"]')];
        const current = tabs.indexOf(tab);
        if (current < 0 || tabs.length === 0) {
          return;
        }
        const next =
          event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : (current + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) %
                tabs.length;
        event.preventDefault();
        tabs[next].focus();
        tabs[next].click();
        return;
      }
      const focusables = visibleFocusables(activeDialog);
      if (focusables.length === 0) {
        event.preventDefault();
        activeDialog.focus();
        return;
      }
      const first = focusables[0];
      const last = focusables.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", startInteractions, { once: true });
  } else {
    startInteractions();
  }
})();
