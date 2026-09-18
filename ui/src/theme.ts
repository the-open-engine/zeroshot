export type ThemePreference = 'system' | 'light' | 'dark';

const storageKey = 'zeroshot.theme.v1';
let preference: ThemePreference = 'system';
let media: MediaQueryList | undefined;
let initialized = false;
const listeners = new Set<() => void>();

function parsePreference(value: string | null): ThemePreference {
  return value === 'light' || value === 'dark' ? value : 'system';
}

function applyTheme() {
  document.documentElement.dataset.theme =
    preference === 'system' ? (media?.matches ? 'dark' : 'light') : preference;
}

function notify() {
  applyTheme();
  listeners.forEach((listener) => listener());
}

/** Call before mounting React so the first application paint uses the device preference. */
export function initializeTheme() {
  if (initialized || typeof window === 'undefined' || typeof document === 'undefined') return;
  initialized = true;
  try {
    preference = parsePreference(window.localStorage.getItem(storageKey));
  } catch {
    // A blocked or full storage area must never prevent using the application.
  }
  if (typeof window.matchMedia === 'function') {
    media = window.matchMedia('(prefers-color-scheme: dark)');
    media.addEventListener('change', () => {
      if (preference === 'system') applyTheme();
    });
  }
  window.addEventListener('storage', (event) => {
    if (event.key !== storageKey && event.key !== null) return;
    try {
      if (event.storageArea && event.storageArea !== window.localStorage) return;
    } catch {
      return;
    }
    preference = parsePreference(event.newValue);
    notify();
  });
  applyTheme();
}

export function getThemePreference(): ThemePreference {
  return preference;
}

export function subscribeTheme(listener: () => void) {
  initializeTheme();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function setThemePreference(value: ThemePreference) {
  initializeTheme();
  preference = parsePreference(value);
  try {
    window.localStorage.setItem(storageKey, preference);
  } catch {
    // The selection remains usable for this page when persistence is unavailable.
  }
  notify();
}
