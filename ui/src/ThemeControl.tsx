import { useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';
import { Monitor, Moon, Sun } from 'lucide-react';
import {
  getThemePreference,
  setThemePreference,
  subscribeTheme,
  type ThemePreference,
} from './theme';

const choices = [
  { value: 'system', label: 'System theme', Icon: Monitor },
  { value: 'light', label: 'Light theme', Icon: Sun },
  { value: 'dark', label: 'Dark theme', Icon: Moon },
] satisfies { value: ThemePreference; label: string; Icon: typeof Monitor }[];

export function ThemeControl() {
  const preference = useSyncExternalStore(subscribeTheme, getThemePreference, () => 'system');
  const [open, setOpen] = useState(false);
  const control = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const selected = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const Icon = preference === 'system' ? Monitor : preference === 'dark' ? Moon : Sun;

  useEffect(() => {
    if (!open) return;
    selected.current?.focus();
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && !control.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, [open]);

  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };

  return (
    <div
      className="theme-control"
      ref={control}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
      onKeyDown={(event) => {
        if (open && event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          close();
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        className="theme-trigger"
        aria-label={`Color theme: ${preference}`}
        title={`Color theme: ${preference}`}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen(!open)}
      >
        <Icon size={16} strokeWidth={1.8} aria-hidden="true" />
      </button>
      {open && (
        <div id={panelId} className="theme-choices" role="group" aria-label="Color theme">
          {choices.map(({ value, label, Icon: ChoiceIcon }) => (
            <button
              key={value}
              ref={preference === value ? selected : undefined}
              type="button"
              aria-label={label}
              title={label}
              aria-pressed={preference === value}
              onClick={() => {
                setThemePreference(value);
                close();
              }}
            >
              <ChoiceIcon size={15} strokeWidth={1.8} aria-hidden="true" />
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
