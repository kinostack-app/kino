import { TriangleAlert } from 'lucide-react';
import { useRef } from 'react';
import { createPortal } from 'react-dom';
import { useModalA11y } from '@/hooks/useModalA11y';

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: string;
  confirmLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = 'Remove',
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const { titleId, descriptionId, dialogProps } = useModalA11y({
    open,
    onClose: onCancel,
    containerRef: contentRef,
  });

  if (!open) return null;

  return createPortal(
    // The backdrop is a visual element with a click-to-dismiss
    // affordance — explicitly `role="presentation"` so biome's
    // a11y lints don't flag it as an interactive non-button. The
    // dialog role + aria attrs live on the inner content node.
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click is a visual dismiss; keyboard dismissal is handled by useModalA11y
    <div
      ref={overlayRef}
      role="presentation"
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => {
        if (e.target === overlayRef.current) onCancel();
      }}
    >
      <div
        ref={contentRef}
        className="bg-[var(--bg-secondary)] rounded-xl p-6 max-w-sm w-full border border-white/10 shadow-2xl"
        {...dialogProps}
      >
        <div className="flex items-start gap-3 mb-4">
          <div className="w-10 h-10 rounded-full bg-red-500/10 grid place-items-center flex-shrink-0 mt-0.5">
            <TriangleAlert size={20} className="text-red-400" aria-hidden="true" />
          </div>
          <div>
            <h3 id={titleId} className="font-semibold text-white">
              {title}
            </h3>
            <p id={descriptionId} className="text-sm text-[var(--text-muted)] mt-1">
              {description}
            </p>
          </div>
        </div>
        <div className="flex items-center justify-end gap-3">
          <button
            type="button"
            onClick={onCancel}
            className="px-4 py-2 rounded-lg bg-white/10 hover:bg-white/15 text-sm font-medium transition"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="px-4 py-2 rounded-lg bg-red-600 hover:bg-red-500 text-white text-sm font-semibold transition"
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}
