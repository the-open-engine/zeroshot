import { Children, cloneElement, isValidElement, useId } from 'react';
export function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: React.ReactNode;
  hint?: string;
}) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      {Children.map(children, (child) =>
        isValidElement(child) &&
        typeof child.type === 'string' &&
        ['input', 'select', 'textarea'].includes(child.type)
          ? cloneElement(child as React.ReactElement<{ id?: string }>, { id })
          : child
      )}
      {hint && <small>{hint}</small>}
    </div>
  );
}
