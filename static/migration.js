// Keep the old origin and browser profile: paths alone cannot recover localStorage.
const keys = ["session", "settings", "activeTab", "hiddenColumns", "columnOrder", "columnWidths", "tableHeight", "timeSpan", "compare", "checkUpdatesAtStart", "mcpServer"];
const values = Object.fromEntries(keys.map((key) => [`traceplotter.${key}`, localStorage.getItem(`traceplotter.${key}`)]));
window.__TAURI_INTERNALS__.invoke("store_migration", { values }).catch((error) => {
  window.__TAURI_INTERNALS__.invoke("migration_failed", { error: String(error) });
});
