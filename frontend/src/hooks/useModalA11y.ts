/**
 * Focus trap + focus restore + labelledby helper for hand-rolled
 * modals. Not a full replacement for Radix / Headless UI — just
 * enough to meet the a11y spec for our two modals
 * (`ConfirmDialog`, `AddListModal`) without a dep change.
 *
 * Behaviours:
 *   - On open: save `document.activeElement`, focus the first
 *     focusable in the container.
 *   - On close / unmount: restore focus to the saved element.
 *   - Tab / Shift+Tab cycles inside the container (standard trap).
 *
 * Intended usage:
 *   const { dialogProps, titleId } = useModalA11y({ open, onClose });
 *   return (
 *     <div {...dialogProps}>
 *       <h3 id={titleId}>...</h3>
 *       ...
 *     </div>
 *   );
 */

import { type RefObject, useCallback, useEffect, useId, useRef } from 'react';

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function useModalA11y({
  open,
  onClose,
  containerRef,
  initialFocusRef,
}: {
  open: boolean;
  onClose: () => void;
  containerRef: RefObject<HTMLElement | null>;
  /** When set, focus this element on open instead of the first
   *  focusable in the container. Use for dialogs where the primary
   *  affordance isn't the top-left close button (e.g. the URL input
   *  in `AddListModal`). */
  initialFocusRef?: RefObject<HTMLElement | null>;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const previouslyFocused = useRef<HTMLElement | null>(null);

  // Capture the element that was focused before the dialog opened so
  // we can restore focus when it closes — matches the behaviour
  // users expect from native `<dialog>` elements. Refs are stable;
  // depending on them would force a re-bind every render.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refs are stable handles
  useEffect(() => {
    if (!open) return;
    previouslyFocused.current = document.activeElement as HTMLElement | null;
    // Defer focus to the next microtask so the portal + dialog
    // content are mounted before we try to focus.
    const raf = requestAnimationFrame(() => {
      const explicit = initialFocusRef?.current;
      if (explicit) {
        explicit.focus();
        return;
      }
      const node = containerRef.current;
      if (!node) return;
      const first = node.querySelector<HTMLElement>(FOCUSABLE_SELECTOR);
      if (first) {
        first.focus();
      } else {
        // Fall back to the container itself so screen readers still
        // land on the dialog rather than remaining on the opener.
        node.setAttribute('tabindex', '-1');
        node.focus();
      }
    });
    return () => {
      cancelAnimationFrame(raf);
      previouslyFocused.current?.focus();
    };
  }, [open, containerRef]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== 'Tab') return;
      const node = containerRef.current;
      if (!node) return;
      const focusable = Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
      if (focusable.length === 0) {
        e.preventDefault();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [onClose, containerRef]
  );

  return {
    titleId,
    descriptionId,
    dialogProps: {
      role: 'dialog' as const,
      'aria-modal': true as const,
      'aria-labelledby': titleId,
      'aria-describedby': descriptionId,
      onKeyDown,
    },
  };
}
