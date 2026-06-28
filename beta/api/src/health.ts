import {
  createCanvasSurface,
  renderSurfaceProbe,
  type DeepHealthResponse,
} from '@beta-uss-earth/types';

export function runDeepHealthCheck(now = new Date()): DeepHealthResponse {
  const surface = createCanvasSurface();
  const probe = renderSurfaceProbe(surface);
  const checks = {
    hasCanvasPane:
      probe.paneCount === 1 && surface.panes[0].id === 'primary-canvas',
    hasTextEntry: probe.entryKind === 'text' && surface.entry.name === 'entry',
    hasPixelBudget:
      surface.panes[0].pixelBudget ===
      surface.panes[0].width * surface.panes[0].height,
  };

  return {
    ok: Object.values(checks).every(Boolean),
    service: 'beta-api',
    checkedAt: now.toISOString(),
    probe,
    checks,
  };
}
