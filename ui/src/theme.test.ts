import test, { type TestContext } from 'node:test';
import assert from 'node:assert/strict';

const storageKey = 'zeroshot.theme.v1';
let moduleId = 0;

async function browser(
  t: TestContext,
  options: {
    dark?: boolean;
    saved?: string;
    storageBlocked?: boolean;
    writesBlocked?: boolean;
  } = {}
) {
  const values = new Map<string, string>();
  if (options.saved !== undefined) values.set(storageKey, options.saved);
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      if (options.writesBlocked) throw new DOMException('Storage full', 'QuotaExceededError');
      values.set(key, value);
    },
  };
  const media = Object.assign(new EventTarget(), { matches: options.dark ?? false });
  const window = Object.assign(new EventTarget(), {
    matchMedia: (query: string) => {
      assert.equal(query, '(prefers-color-scheme: dark)');
      return media;
    },
  });
  Object.defineProperty(window, 'localStorage', {
    get() {
      if (options.storageBlocked) throw new DOMException('Storage blocked', 'SecurityError');
      return storage;
    },
  });
  const document = { documentElement: { dataset: {} as Record<string, string> } };
  for (const [name, value] of Object.entries({ window, document })) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => {
      if (original) Object.defineProperty(globalThis, name, original);
      else Reflect.deleteProperty(globalThis, name);
    });
  }
  // Each simulated page gets its own module state without adding a reset API to production code.
  const url = new URL('./theme.ts', import.meta.url);
  url.searchParams.set('test', String(moduleId++));
  const theme: typeof import('./theme') = await import(url.href);
  return {
    theme,
    resolved: () => document.documentElement.dataset.theme,
    stored: () => values.get(storageKey),
    systemDark(dark: boolean) {
      media.matches = dark;
      media.dispatchEvent(new Event('change'));
    },
    storageChanged(key: string | null, newValue: string | null, storageArea: object = storage) {
      if (storageArea === storage) {
        if (key === null) values.clear();
        else if (newValue === null) values.delete(key);
        else values.set(key, newValue);
      }
      window.dispatchEvent(Object.assign(new Event('storage'), { key, newValue, storageArea }));
    },
  };
}

test('theme defaults to the dark system appearance before React subscribes', async (t) => {
  const page = await browser(t, { dark: true });
  page.theme.initializeTheme();
  assert.equal(page.theme.getThemePreference(), 'system');
  assert.equal(page.resolved(), 'dark');
  assert.equal(page.stored(), undefined);
});

test('system preference follows live operating system appearance changes', async (t) => {
  const page = await browser(t, { dark: true });
  page.theme.initializeTheme();
  page.systemDark(false);
  assert.equal(page.resolved(), 'light');
  page.systemDark(true);
  assert.equal(page.resolved(), 'dark');
  assert.equal(page.theme.getThemePreference(), 'system');
});

for (const preference of ['light', 'dark'] as const) {
  test(`saved ${preference} preference overrides system changes until System is selected`, async (t) => {
    const page = await browser(t, { dark: preference === 'light', saved: preference });
    page.theme.initializeTheme();
    assert.equal(page.theme.getThemePreference(), preference);
    assert.equal(page.resolved(), preference);
    page.systemDark(false);
    page.systemDark(true);
    assert.equal(page.resolved(), preference);

    page.theme.setThemePreference('system');
    assert.equal(page.stored(), 'system');
    assert.equal(page.resolved(), 'dark');
    page.systemDark(false);
    assert.equal(page.resolved(), 'light');

    page.theme.setThemePreference(preference);
    assert.equal(page.stored(), preference);
    page.systemDark(preference === 'light');
    assert.equal(page.resolved(), preference);
  });
}

test('cross-tab preference changes and clearing storage update the theme and subscribers', async (t) => {
  const page = await browser(t, { dark: true });
  page.theme.initializeTheme();
  const observed: string[] = [];
  const unsubscribe = page.theme.subscribeTheme(() =>
    observed.push(page.theme.getThemePreference())
  );

  page.storageChanged(storageKey, 'light');
  assert.equal(page.resolved(), 'light');
  page.storageChanged('unrelated-key', 'dark');
  page.storageChanged(storageKey, 'dark', {});
  assert.equal(page.resolved(), 'light');
  assert.deepEqual(observed, ['light']);

  page.storageChanged(storageKey, 'dark');
  assert.equal(page.resolved(), 'dark');
  page.storageChanged(null, null);
  assert.equal(page.theme.getThemePreference(), 'system');
  assert.equal(page.resolved(), 'dark');
  page.systemDark(false);
  assert.equal(page.resolved(), 'light');
  assert.deepEqual(observed, ['light', 'dark', 'system']);

  unsubscribe();
  page.storageChanged(storageKey, 'dark');
  assert.equal(page.resolved(), 'dark');
  assert.deepEqual(observed, ['light', 'dark', 'system']);
});

test('blocked storage still permits system appearance and manual selection', async (t) => {
  const page = await browser(t, { dark: true, storageBlocked: true });
  assert.doesNotThrow(() => page.theme.initializeTheme());
  assert.equal(page.resolved(), 'dark');
  assert.doesNotThrow(() => page.theme.setThemePreference('light'));
  assert.equal(page.theme.getThemePreference(), 'light');
  page.systemDark(true);
  assert.equal(page.resolved(), 'light');
});

test('a failed storage write keeps the selected theme usable for the current page', async (t) => {
  const page = await browser(t, { saved: 'dark', writesBlocked: true });
  page.theme.initializeTheme();
  assert.equal(page.resolved(), 'dark');
  assert.doesNotThrow(() => page.theme.setThemePreference('light'));
  assert.equal(page.theme.getThemePreference(), 'light');
  assert.equal(page.resolved(), 'light');
  assert.equal(page.stored(), 'dark');
});
