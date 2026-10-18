import {
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
  type RefObject,
} from 'react';

const storageKey = 'zeroshot.inspector-width.v1';
let preferredWidth: number | undefined;
let storageRead = false;
const listeners = new Set<() => void>();
function getPreferredWidth() {
  if (!storageRead && typeof window !== 'undefined') {
    storageRead = true;
    try {
      const stored = Number(window.localStorage.getItem(storageKey));
      if (Number.isFinite(stored) && stored > 0) preferredWidth = stored;
    } catch {
      // Resizing remains available when browser storage is blocked.
    }
  }
  return preferredWidth;
}
function subscribeWidth(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
function saveWidth(width: number) {
  preferredWidth = width;
  try {
    window.localStorage.setItem(storageKey, String(width));
  } catch {
    // Keep the preference for this page if persistence is unavailable.
  }
  listeners.forEach((listener) => listener());
}
const clamp = (width: number, min: number, max: number) =>
  Math.round(Math.max(min, Math.min(max, width)));

/** Shared right-hand inspector geometry; a display preference never enters a profile or run. */
export function InspectorPanel({
  children,
  className,
  label,
}: {
  children: ReactNode;
  className: string;
  label: string;
}) {
  const id = useId();
  const panel = useRef<HTMLElement>(null);
  const drag = useRef<{ pointerId: number; startX: number; width: number } | null>(null);
  const preference = useSyncExternalStore(subscribeWidth, getPreferredWidth, () => undefined);
  const [dragWidth, setDragWidth] = useState<number>();
  const bounds = useInspectorBounds(panel);
  const width = clamp(dragWidth ?? preference ?? bounds.defaultWidth, bounds.min, bounds.max);
  return (
    <aside
      ref={panel}
      id={id}
      className={`inspector-panel ${className}`}
      aria-label={label}
      style={{ width }}
    >
      <div
        className="inspector-resize-handle"
        role="separator"
        tabIndex={0}
        aria-label={`Resize ${label.toLowerCase()}`}
        aria-controls={id}
        aria-orientation="vertical"
        aria-valuemin={bounds.min}
        aria-valuemax={bounds.max}
        aria-valuenow={width}
        aria-valuetext={`${width} pixels wide`}
        title="Drag to resize. Left and Right arrows adjust width; Home and End use the limits."
        onKeyDown={(event) => {
          const step = event.shiftKey ? 64 : 16;
          const next = {
            ArrowLeft: width + step,
            ArrowRight: width - step,
            Home: bounds.min,
            End: bounds.max,
          }[event.key];
          if (next === undefined) return;
          event.preventDefault();
          saveWidth(clamp(next, bounds.min, bounds.max));
        }}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          event.currentTarget.focus();
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { pointerId: event.pointerId, startX: event.clientX, width };
        }}
        onPointerMove={(event) => {
          const start = drag.current;
          if (start?.pointerId !== event.pointerId) return;
          setDragWidth(clamp(start.width + start.startX - event.clientX, bounds.min, bounds.max));
        }}
        onPointerUp={(event) => {
          const start = drag.current;
          if (start?.pointerId !== event.pointerId) return;
          saveWidth(clamp(start.width + start.startX - event.clientX, bounds.min, bounds.max));
          drag.current = null;
          setDragWidth(undefined);
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onLostPointerCapture={() => {
          drag.current = null;
          setDragWidth(undefined);
        }}
        onPointerCancel={() => {
          drag.current = null;
          setDragWidth(undefined);
        }}
      />
      {children}
    </aside>
  );
}

function useInspectorBounds(panel: RefObject<HTMLElement | null>) {
  const [bounds, setBounds] = useState({ min: 280, max: 800, defaultWidth: 322 });
  useEffect(() => {
    const element = panel.current!;
    const parent = element.parentElement!;
    function measure() {
      const available = Math.min(parent.getBoundingClientRect().width, window.innerWidth);
      if (!available) return; // Hidden views retain their last usable geometry.
      const style = window.getComputedStyle(element);
      const overlay = style.position === 'absolute';
      const max = Math.max(
        1,
        Math.floor(overlay ? available - 24 : Math.min(available * 0.7, available - 240))
      );
      setBounds({
        min: Math.min(280, max),
        max,
        defaultWidth: Number.parseFloat(style.getPropertyValue('--inspector-default-width')) || 322,
      });
    }
    measure();
    const observer =
      typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure);
    observer?.observe(parent);
    window.addEventListener('resize', measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener('resize', measure);
    };
  }, []);
  return bounds;
}
