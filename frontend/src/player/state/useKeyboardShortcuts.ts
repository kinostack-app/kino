import { useEffect } from 'react';

/**
 * Global keyboard shortcut handler. Attached at window
 * level, filtered against typing contexts so we don't
 * hijack users editing in `<input>`/`<textarea>`/
 * contenteditable/ARIA textbox/searchbox/combobox.
 *
 * The handler receives a normalized `key` string (e.g.
 * "Space" instead of " ", "ArrowLeft", "c"), which keeps
 * the consumer's switch readable.
 */
/** Return `true` if the handler consumed the keypress
 *  (player will call `preventDefault`); `false`/undefined
 *  lets the event bubble. */
export type KeyboardShortcutHandler = (key: string, event: KeyboardEvent) => boolean | undefined;

export function useKeyboardShortcuts(handler: KeyboardShortcutHandler) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Never hijack modifier chords — Cmd/Ctrl/Alt belong
      // to the browser or OS, not to the player.
      if (e.ctrlKey || e.metaKey || e.altKey) return;

      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
        if (target.isContentEditable) return;
        const role = target.getAttribute('role');
        if (role === 'textbox' || role === 'searchbox' || role === 'combobox') return;
      }

      const normalized = e.key === ' ' ? 'Space' : e.key;
      const consumed = handler(normalized, e);
      if (consumed) e.preventDefault();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [handler]);
}
