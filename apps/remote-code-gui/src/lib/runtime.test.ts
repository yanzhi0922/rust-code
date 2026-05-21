import { afterEach, describe, expect, it } from 'vitest';
import { hasTauriRuntime, shouldUseRemoteMode } from './runtime';

function setTauriRuntime(enabled: boolean) {
  Object.defineProperty(window, '__TAURI__', {
    configurable: true,
    value: enabled ? {} : undefined,
  });
  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: undefined,
  });
}

describe('runtime mode detection', () => {
  afterEach(() => {
    setTauriRuntime(false);
    window.history.pushState({}, '', '/');
  });

  it('does not enable remote mode in a regular browser even when query params request it', () => {
    setTauriRuntime(false);
    window.history.pushState({}, '', '/?mode=remote&control_plane_url=https%3A%2F%2Fremote-code.yz520gzy.top');

    expect(hasTauriRuntime()).toBe(false);
    expect(shouldUseRemoteMode()).toBe(false);
  });

  it('enables remote mode only for native runtime with mode=remote', () => {
    setTauriRuntime(true);
    window.history.pushState({}, '', '/?mode=remote');

    expect(hasTauriRuntime()).toBe(true);
    expect(shouldUseRemoteMode()).toBe(true);
  });

  it('keeps native runtime in local mode unless remote mode is explicit', () => {
    setTauriRuntime(true);
    window.history.pushState({}, '', '/');

    expect(shouldUseRemoteMode()).toBe(false);
  });
});
