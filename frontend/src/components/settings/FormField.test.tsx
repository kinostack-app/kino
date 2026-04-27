import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TestButton } from './FormField';

describe('TestButton', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
  });

  // State assertions go through `data-state` because the label text
  // is intentionally static across states — the visual change is
  // icon + colour only.
  const stateOf = () => screen.getByRole('button').getAttribute('data-state');

  it('holds the testing state for at least the minimum visible duration', async () => {
    // Resolves immediately — without the min-duration the spinner
    // would flash for ~0ms and the user wouldn't see it.
    const onTest = vi.fn().mockResolvedValue(true);
    render(<TestButton onTest={onTest} label="Probe" />);

    const button = screen.getByRole('button', { name: /probe/i });
    await act(async () => {
      button.click();
    });

    expect(stateOf()).toBe('testing');

    // Advance just under the floor — still testing.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(299);
    });
    expect(stateOf()).toBe('testing');

    // Cross the floor — now settles to success.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(stateOf()).toBe('success');
  });

  it('stays on failed after rejection (no auto-revert)', async () => {
    const onTest = vi.fn().mockResolvedValue(false);
    render(<TestButton onTest={onTest} label="Probe" />);

    await act(async () => {
      screen.getByRole('button').click();
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(stateOf()).toBe('failed');

    // Even 10 seconds later the state hasn't reverted — the old
    // component auto-reverted after 3s, which caused layout shift
    // and lost the information the user just read.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(stateOf()).toBe('failed');
  });

  it('treats thrown errors as a failed test', async () => {
    const onTest = vi.fn().mockRejectedValue(new Error('boom'));
    render(<TestButton onTest={onTest} label="Probe" />);

    await act(async () => {
      screen.getByRole('button').click();
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(stateOf()).toBe('failed');
  });

  it('keeps the label static across states (only icon changes)', async () => {
    const onTest = vi.fn().mockResolvedValue(true);
    render(<TestButton onTest={onTest} label="Probe" />);

    // Idle — label visible.
    expect(screen.getByRole('button').textContent).toContain('Probe');

    await act(async () => {
      screen.getByRole('button').click();
    });
    expect(screen.getByRole('button').textContent).toContain('Probe'); // during testing

    await act(async () => {
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(screen.getByRole('button').textContent).toContain('Probe'); // after success
  });
});
