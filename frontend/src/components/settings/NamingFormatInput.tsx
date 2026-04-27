import * as Popover from '@radix-ui/react-popover';
import { AlertCircle, Plus } from 'lucide-react';
import { useId, useRef } from 'react';
import { cn } from '@/lib/utils';

/**
 * Text input specialised for filename templates. Wraps the standard
 * TextInput with:
 *   • A "+ Token" popover that inserts a `{token}` at the cursor.
 *   • Inline validation of any tokens the user typed manually — anything
 *     not in `validTokens` is flagged.
 *   • A "missing required" warning when a format omits tokens we need
 *     to disambiguate output (e.g. movies without `{title}`).
 */
interface NamingFormatInputProps {
  value: string;
  onChange: (value: string) => void;
  /** All tokens accepted for this format. Order is shown in the popover. */
  validTokens: string[];
  /** Tokens that must appear somewhere in the format. */
  requiredTokens?: string[];
  placeholder?: string;
}

const TOKEN_RE = /\{([a-zA-Z_][\w]*)(?::\d+)?\}/g;

function parseTokens(format: string): string[] {
  const found: string[] = [];
  let m: RegExpExecArray | null = TOKEN_RE.exec(format);
  while (m !== null) {
    found.push(m[1]);
    m = TOKEN_RE.exec(format);
  }
  return found;
}

export function NamingFormatInput({
  value,
  onChange,
  validTokens,
  requiredTokens,
  placeholder,
}: NamingFormatInputProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const inputId = useId();

  const used = parseTokens(value);
  const validSet = new Set(validTokens);
  const unknown = [...new Set(used.filter((t) => !validSet.has(t)))];
  const missing = requiredTokens?.filter((t) => !used.includes(t)) ?? [];

  const insert = (token: string) => {
    const el = inputRef.current;
    if (!el) {
      onChange(`${value}{${token}}`);
      return;
    }
    const start = el.selectionStart ?? value.length;
    const end = el.selectionEnd ?? value.length;
    const snippet = `{${token}}`;
    const next = value.slice(0, start) + snippet + value.slice(end);
    onChange(next);
    // Restore the cursor after the inserted token on the next tick.
    requestAnimationFrame(() => {
      el.focus();
      const cursor = start + snippet.length;
      el.setSelectionRange(cursor, cursor);
    });
  };

  return (
    <div>
      <div className="relative">
        <input
          id={inputId}
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          spellCheck={false}
          className={cn(
            'w-full h-9 pl-3 pr-10 rounded-lg bg-[var(--bg-card)] border text-sm text-white placeholder:text-[var(--text-muted)] font-mono',
            'focus:outline-none focus:ring-1 focus:ring-[var(--accent)]',
            unknown.length > 0 ? 'border-red-500/50' : 'border-white/10'
          )}
        />
        <Popover.Root>
          <Popover.Trigger asChild>
            <button
              type="button"
              className="absolute right-1 top-1/2 -translate-y-1/2 h-7 px-2 rounded-md text-[var(--text-muted)] hover:text-white hover:bg-white/5 transition flex items-center gap-1 text-xs"
              aria-label="Insert token"
            >
              <Plus size={12} />
              Token
            </button>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              side="bottom"
              align="end"
              sideOffset={6}
              collisionPadding={16}
              className="z-50 w-56 rounded-lg border border-white/10 bg-[var(--bg-card)] p-1 shadow-xl data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
            >
              <div className="max-h-72 overflow-auto">
                {validTokens.map((t) => (
                  <Popover.Close asChild key={t}>
                    <button
                      type="button"
                      onClick={() => insert(t)}
                      className="w-full flex items-center justify-between gap-2 px-2 py-1.5 rounded-md text-xs text-[var(--text-secondary)] hover:bg-white/5 hover:text-white transition font-mono"
                    >
                      <span>{`{${t}}`}</span>
                      {used.includes(t) && (
                        <span className="text-[10px] text-[var(--text-muted)]">in use</span>
                      )}
                    </button>
                  </Popover.Close>
                ))}
              </div>
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>
      </div>

      {unknown.length > 0 && (
        <p className="mt-1 flex items-start gap-1 text-xs text-red-400">
          <AlertCircle size={12} className="flex-shrink-0 mt-0.5" />
          <span>
            Unknown token{unknown.length === 1 ? '' : 's'}:{' '}
            <span className="font-mono">{unknown.map((t) => `{${t}}`).join(', ')}</span>
          </span>
        </p>
      )}
      {unknown.length === 0 && missing.length > 0 && (
        <p className="mt-1 flex items-start gap-1 text-xs text-amber-400">
          <AlertCircle size={12} className="flex-shrink-0 mt-0.5" />
          <span>
            Missing recommended token{missing.length === 1 ? '' : 's'}:{' '}
            <span className="font-mono">{missing.map((t) => `{${t}}`).join(', ')}</span>
          </span>
        </p>
      )}
    </div>
  );
}
