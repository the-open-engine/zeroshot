import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { MoreHorizontal } from 'lucide-react';

export type MoreMenuItem = {
  label: string;
  icon: ReactNode;
  onSelect: () => void;
  /** Unavailable but still focusable, like a toolbar toggle that just did its work. */
  disabled?: boolean;
  /** Lets a page's CSS show an item only at some widths; hidden items are skipped by the keys. */
  className?: string;
};

const keyMoves: Record<string, (index: number, count: number) => number> = {
  ArrowDown: (index, count) => (index + 1) % count,
  ArrowUp: (index, count) => (index - 1 + count) % count,
  Home: () => 0,
  End: (_, count) => count - 1,
};

/**
 * Tools that no longer fit beside the others, folded behind one "More" button. It closes on a
 * choice, Escape, a click anywhere else, or focus moving away.
 */
export function MoreMenu({ label, items }: { label: string; items: readonly MoreMenuItem[] }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const shownItems = () =>
    Array.from(list.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []).filter(
      (item) => window.getComputedStyle(item).display !== 'none'
    );

  useEffect(() => {
    if (!open) return;
    shownItems()[0]?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);

  // Focus goes back to the trigger before an item acts, so a dialog it opens can return there.
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!open) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    const move = keyMoves[event.key];
    const shown = shownItems();
    if (!move || !shown.length) return;
    event.preventDefault();
    shown[move(shown.indexOf(document.activeElement as HTMLElement), shown.length)].focus();
  };

  return (
    <div
      className="more-menu"
      ref={root}
      onKeyDown={keyDown}
      onBlur={(event) => {
        // Pressing a button in Safari clears focus; presses outside close via pointerdown.
        if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget))
          setOpen(false);
      }}
    >
      <button
        ref={trigger}
        type="button"
        className="icon-button"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen(!open)}
      >
        <MoreHorizontal size={17} />
      </button>
      {open && (
        <div id={menuId} ref={list} className="more-menu-list" role="menu" aria-label={label}>
          {items.map((item) => (
            <button
              key={item.label}
              type="button"
              role="menuitem"
              tabIndex={-1}
              className={item.className}
              aria-disabled={item.disabled || undefined}
              onClick={() => {
                if (item.disabled) return;
                close();
                item.onSelect();
              }}
            >
              {item.icon}
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
