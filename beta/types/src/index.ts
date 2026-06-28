export const BETA_SURFACE_VERSION = 'm0.1' as const;

export type BetaSurfaceVersion = typeof BETA_SURFACE_VERSION;
export type BetaRoute = '/';

export type CanvasPaneSpec = Readonly<{
  id: 'primary-canvas';
  label: 'Blank canvas';
  width: number;
  height: number;
  pixelBudget: number;
}>;

export type BottomEntrySpec = Readonly<{
  id: 'bottom-entry';
  kind: 'text';
  name: 'entry';
}>;

export type BetaSurfaceSpec = Readonly<{
  version: BetaSurfaceVersion;
  route: BetaRoute;
  panes: readonly [CanvasPaneSpec];
  entry: BottomEntrySpec;
}>;

export type SurfaceRenderProbe = Readonly<{
  route: BetaRoute;
  renderKey: string;
  paneCount: number;
  entryKind: BottomEntrySpec['kind'];
}>;

export type DeepHealthResponse = Readonly<{
  ok: boolean;
  service: 'beta-api';
  checkedAt: string;
  probe: SurfaceRenderProbe;
  checks: Readonly<{
    hasCanvasPane: boolean;
    hasTextEntry: boolean;
    hasPixelBudget: boolean;
  }>;
}>;

export function createCanvasSurface(): BetaSurfaceSpec {
  const width = 1920;
  const height = 1080;

  return {
    version: BETA_SURFACE_VERSION,
    route: '/',
    panes: [
      {
        id: 'primary-canvas',
        label: 'Blank canvas',
        width,
        height,
        pixelBudget: width * height,
      },
    ],
    entry: {
      id: 'bottom-entry',
      kind: 'text',
      name: 'entry',
    },
  };
}

export function renderSurfaceProbe(
  surface: BetaSurfaceSpec = createCanvasSurface(),
): SurfaceRenderProbe {
  return {
    route: surface.route,
    renderKey: `${surface.version}:${surface.route}:${surface.panes[0].id}:${surface.entry.kind}`,
    paneCount: surface.panes.length,
    entryKind: surface.entry.kind,
  };
}
