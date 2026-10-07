import { Plus } from 'lucide-react';
export function AddEnvironmentField({
  children,
  onClick,
}: {
  children: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="text-button" onClick={onClick}>
      <Plus size={14} />
      {children}
    </button>
  );
}
