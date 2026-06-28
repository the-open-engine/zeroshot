import { createCanvasSurface } from '@beta-uss-earth/types';

const surface = createCanvasSurface();
const canvas = surface.panes[0];

export function App() {
  return (
    <main class="beta-shell" aria-label="beta.uss.earth canvas">
      <section class="canvas-pane" aria-label={canvas.label}>
        <canvas
          class="canvas-surface"
          width={canvas.width}
          height={canvas.height}
          aria-label={canvas.label}
        />
      </section>
      <form class="entry-pane" action="/" method="get">
        <input
          id={surface.entry.id}
          name={surface.entry.name}
          type={surface.entry.kind}
          autocomplete="off"
          aria-label="Text entry"
        />
      </form>
    </main>
  );
}
