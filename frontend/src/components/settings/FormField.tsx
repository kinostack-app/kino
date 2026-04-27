import * as Popover from '@radix-ui/react-popover';
import {
  AlertCircle,
  Check,
  Eye,
  EyeOff,
  HelpCircle,
  Loader2,
  Play,
  RotateCcw,
} from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { cn } from '@/lib/utils';

// ── FormField wrapper ──

interface FormFieldProps {
  label: string;
  description?: string;
  help?: string;
  error?: string;
  preview?: string;
  defaultValue?: string;
  onReset?: () => void;
  children: ReactNode;
}

export function FormField({
  label,
  description,
  help,
  error,
  preview,
  defaultValue,
  onReset,
  children,
}: FormFieldProps) {
  return (
    <div className="py-3">
      <div className="flex flex-col sm:flex-row sm:items-start gap-1 sm:gap-4">
        <div className="sm:w-44 flex-shrink-0">
          <div className="flex items-center gap-1.5">
            <span className="text-sm font-medium text-[var(--text-primary)]">{label}</span>
            {help && (
              // Help text lives in a floating popover rather than inline
              // prose — opening it used to push the whole form down. The
              // popover is anchored to the `?` icon, floats over content,
              // and stays usable on touch (click, not hover).
              <Popover.Root>
                <Popover.Trigger asChild>
                  <button
                    type="button"
                    className="text-[var(--text-muted)] hover:text-white transition"
                    aria-label={`Help for ${label}`}
                  >
                    <HelpCircle size={13} />
                  </button>
                </Popover.Trigger>
                <Popover.Portal>
                  <Popover.Content
                    side="right"
                    align="start"
                    sideOffset={8}
                    collisionPadding={16}
                    className="z-50 max-w-xs rounded-lg border border-white/10 bg-[var(--bg-card)] p-3 text-xs text-[var(--text-secondary)] leading-relaxed shadow-xl data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
                  >
                    {help}
                    <Popover.Arrow className="fill-[var(--bg-card)]" />
                  </Popover.Content>
                </Popover.Portal>
              </Popover.Root>
            )}
            {onReset && defaultValue !== undefined && (
              <button
                type="button"
                onClick={onReset}
                className="text-[var(--text-muted)] hover:text-white transition"
                title={`Reset to: ${defaultValue}`}
              >
                <RotateCcw size={11} />
              </button>
            )}
          </div>
          {description && <p className="text-xs text-[var(--text-muted)] mt-0.5">{description}</p>}
        </div>
        <div className="flex-1 min-w-0">
          {children}
          {error && (
            <p className="flex items-center gap-1 mt-1 text-xs text-red-400">
              <AlertCircle size={12} />
              {error}
            </p>
          )}
          {preview && !error && (
            <p className="mt-1 text-xs text-[var(--text-muted)] font-mono truncate">
              Preview: {preview}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

// ── TextInput ──

interface TextInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: 'text' | 'number' | 'url';
  disabled?: boolean;
  error?: boolean;
  onBlur?: () => void;
  /**
   * Focus this input on mount. Used by modal flows where the field is
   * the obvious next action (e.g. "Name" on the Torznab form).
   */
  autoFocus?: boolean;
}

export function TextInput({
  value,
  onChange,
  placeholder,
  type = 'text',
  disabled,
  error,
  onBlur,
  autoFocus,
}: TextInputProps) {
  return (
    <input
      // biome-ignore lint/a11y/noAutofocus: deliberate in modal step transitions
      autoFocus={autoFocus}
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      placeholder={placeholder}
      disabled={disabled}
      className={cn(
        'w-full h-9 px-3 rounded-lg bg-[var(--bg-card)] border text-sm text-white placeholder:text-[var(--text-muted)]',
        'focus:outline-none focus:ring-1 focus:ring-[var(--accent)] focus:border-[var(--accent)]',
        'disabled:opacity-50 disabled:cursor-not-allowed',
        error ? 'border-red-500/50' : 'border-white/10'
      )}
    />
  );
}

// ── SecretInput ──

interface SecretInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  masked?: boolean;
}

export function SecretInput({ value, onChange, placeholder, masked }: SecretInputProps) {
  const [visible, setVisible] = useState(false);
  const isMasked = masked && value === '***';

  return (
    <div className="relative">
      <input
        type={visible ? 'text' : 'password'}
        value={isMasked ? '' : value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={isMasked ? '••••••••' : placeholder}
        className="w-full h-9 px-3 pr-10 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
      />
      <button
        type="button"
        onClick={() => setVisible(!visible)}
        className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-[var(--text-muted)] hover:text-white"
      >
        {visible ? <EyeOff size={14} /> : <Eye size={14} />}
      </button>
    </div>
  );
}

// ── Toggle ──

interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}

export function Toggle({ checked, onChange, disabled }: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-6 w-11 items-center rounded-full transition-colors',
        checked ? 'bg-[var(--accent)]' : 'bg-white/10',
        disabled && 'opacity-50 cursor-not-allowed'
      )}
    >
      <span
        className={cn(
          'inline-block h-4 w-4 transform rounded-full bg-white transition-transform',
          checked ? 'translate-x-6' : 'translate-x-1'
        )}
      />
    </button>
  );
}

// ── SelectInput ──

interface SelectInputProps {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
}

export function SelectInput({ value, onChange, options }: SelectInputProps) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="h-9 px-3 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)]"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

// ── NumberInput ──

interface NumberInputProps {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  error?: boolean;
  /** Optional unit label (e.g. "GB", "peers"). Rendered inside the
   *  input on the right; reserves padding so the typed number doesn't
   *  overlap. */
  suffix?: string;
}

export function NumberInput({ value, onChange, min, max, step, error, suffix }: NumberInputProps) {
  const input = (
    <input
      type="number"
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      min={min}
      max={max}
      step={step}
      className={cn(
        'h-9 pl-3 rounded-lg bg-[var(--bg-card)] border text-sm text-white',
        suffix ? 'w-32 pr-14' : 'w-full pr-3',
        'focus:outline-none focus:ring-1 focus:ring-[var(--accent)]',
        '[appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none',
        error ? 'border-red-500/50' : 'border-white/10'
      )}
    />
  );
  if (!suffix) return input;
  return (
    <div className="relative inline-flex">
      {input}
      <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-[var(--text-muted)]">
        {suffix}
      </span>
    </div>
  );
}

// ── SpeedInput (human-readable MB/s) ──

interface SpeedInputProps {
  value: number; // bytes/s stored
  onChange: (value: number) => void;
}

export function SpeedInput({ value, onChange }: SpeedInputProps) {
  const mbps = value > 0 ? (value / 1024 / 1024).toFixed(1) : '';

  return (
    <div className="flex items-center gap-2">
      <div className="relative">
        <input
          type="number"
          value={value === 0 ? '' : Number(mbps)}
          onChange={(e) => {
            const v = Number(e.target.value);
            onChange(v > 0 ? Math.round(v * 1024 * 1024) : 0);
          }}
          placeholder="0"
          min={0}
          step={0.1}
          // Right padding reserves space for the "MB/s" suffix so it
          // doesn't overlap the user's typed value.
          className="w-32 h-9 pl-3 pr-14 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)] [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
        />
        <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-[var(--text-muted)]">
          MB/s
        </span>
      </div>
      <span className="text-xs text-[var(--text-muted)]">{value === 0 ? '= unlimited' : ''}</span>
    </div>
  );
}

// ── DurationInput (human-readable) ──

interface DurationInputProps {
  value: number; // stored unit (minutes or hours)
  onChange: (value: number) => void;
  unit: 'minutes' | 'hours';
  min?: number;
}

export function DurationInput({ value, onChange, unit, min = 0 }: DurationInputProps) {
  // Optional "friendlier form" when the exact value rolls over — e.g.
  // 120 minutes → "= 2 hours", 72 hours → "= 3 days". Skipped when it
  // matches the native unit so the label isn't redundant (5 minutes
  // doesn't need "= 5 minutes" tacked on).
  const humanize = () => {
    if (value === 0 && unit === 'minutes') return 'unlimited';
    if (unit === 'hours') {
      if (value >= 24 && value % 24 === 0)
        return `= ${value / 24} day${value / 24 !== 1 ? 's' : ''}`;
      return '';
    }
    if (value >= 60 && value % 60 === 0)
      return `= ${value / 60} hour${value / 60 !== 1 ? 's' : ''}`;
    return '';
  };
  const hint = humanize();
  return (
    <div className="flex items-center gap-2">
      <div className="relative">
        <input
          type="number"
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          min={min}
          className="w-32 h-9 pl-3 pr-20 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white focus:outline-none focus:ring-1 focus:ring-[var(--accent)] [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
        />
        <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-[var(--text-muted)]">
          {unit}
        </span>
      </div>
      {hint && <span className="text-xs text-[var(--text-muted)]">{hint}</span>}
    </div>
  );
}

// ── TestButton ──

/**
 * Fixed-shape button for one-shot connection tests.
 *
 * The label is the user's string at all times — only the leading icon
 * (and the button's colour) changes between states, so the control
 * keeps the same width and the surrounding layout never reflows. The
 * `testing` state holds for at least `MIN_TESTING_MS` so a sub-100ms
 * test doesn't flash through the spinner.
 *
 * The result state is **sticky**: the colour stays until the user
 * clicks again to re-run. An earlier version auto-reverted at 3s,
 * which made the control re-jump and discarded the result the moment
 * the user looked away.
 *
 * The leading icon slot is always reserved (empty in `idle`) so the
 * label position is constant across states.
 */
export type TestButtonState = 'idle' | 'testing' | 'success' | 'failed';

const MIN_TESTING_MS = 300;

interface TestButtonProps {
  onTest: () => Promise<boolean>;
  label?: string;
}

export function TestButton({ onTest, label = 'Test' }: TestButtonProps) {
  const [state, setState] = useState<TestButtonState>('idle');

  const handleTest = async () => {
    setState('testing');
    const start = Date.now();
    let ok = false;
    try {
      ok = await onTest();
    } catch {
      ok = false;
    }
    const elapsed = Date.now() - start;
    if (elapsed < MIN_TESTING_MS) {
      await new Promise((r) => setTimeout(r, MIN_TESTING_MS - elapsed));
    }
    setState(ok ? 'success' : 'failed');
  };

  return (
    <button
      type="button"
      onClick={handleTest}
      disabled={state === 'testing'}
      data-state={state}
      className={cn(
        'inline-flex items-center gap-1.5 h-9 px-3 rounded-lg text-sm font-medium transition flex-shrink-0 ring-1',
        state === 'success' && 'bg-green-500/10 text-green-300 ring-green-500/20',
        state === 'failed' && 'bg-red-500/10 text-red-300 ring-red-500/20',
        state === 'idle' &&
          'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white ring-white/10',
        state === 'testing' && 'bg-white/5 text-[var(--text-muted)] ring-white/10 cursor-progress'
      )}
    >
      <span className="w-3.5 h-3.5 inline-flex items-center justify-center flex-shrink-0">
        {state === 'idle' && <Play size={12} className="fill-current" />}
        {state === 'testing' && <Loader2 size={14} className="animate-spin" />}
        {state === 'success' && <Check size={14} />}
        {state === 'failed' && <AlertCircle size={14} />}
      </span>
      {label}
    </button>
  );
}
