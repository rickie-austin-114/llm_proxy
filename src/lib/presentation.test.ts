import { describe, expect, it } from 'vitest';
import { errorMessage, parsePort } from './presentation';

describe('settings input and error presentation', () => {
  it('rejects privileged, fractional, exponential, and overflowing port input', () => {
    for (const value of ['0', '1023', '65536', '8787.5', '8e3', '', ' 8787 ']) {
      expect(parsePort(value)).toBeNull();
    }
    expect(parsePort('1024')).toBe(1024);
    expect(parsePort('65535')).toBe(65535);
  });

  it('shows structured app errors without exposing arbitrary runtime errors', () => {
    expect(errorMessage({ code: 'invalid_port', message: 'Choose another port.' })).toBe(
      'Choose another port.',
    );
    expect(errorMessage(new Error('sensitive internal details'))).not.toContain('sensitive');
    expect(errorMessage('Bearer fake-test-secret')).not.toContain('fake-test-secret');
  });
});
