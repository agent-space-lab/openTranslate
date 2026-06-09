// Shared theme application used by both the main window and the floating window.
// Each Tauri webview is a separate document, so each window must apply the theme itself.

export function updateAutoTheme(): void {
  const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.setAttribute("data-theme", isDark ? "dark" : "light");
}

export function applyTheme(theme: string): void {
  if (theme === "auto") {
    updateAutoTheme();
  } else {
    document.documentElement.setAttribute("data-theme", theme);
  }
}
