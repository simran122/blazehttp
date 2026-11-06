import { describe, expect, it } from 'vitest';
import {
  ENGINE_ERRORS,
  ERROR_WIRE,
  formatErrorCode,
  H1_LIMITS,
  type ErrorDetailMap,
  type HttpErrorCode,
  METHODS,
  type Method,
} from '../src/generated/spec.ts';

describe('generated spec', () => {
  it('keeps the 15 wire ids in design order', () => {
    const codes: HttpErrorCode[] = ERROR_WIRE.map((e) => e.code);
    expect(codes).toHaveLength(15);
    expect(codes[0]).toBe('TIMEOUT');
    expect(codes[4]).toBe('HTTP_STATUS');
    expect(codes[12]).toBe('QUEUE_FULL');
    expect(codes[14]).toBe('ENGINE');
  });

  it('lists details and field names per code', () => {
    const byCode = new Map(ERROR_WIRE.map((e) => [e.code, e]));
    expect(byCode.get('TIMEOUT')).toEqual({
      code: 'TIMEOUT',
      details: ['connect', 'headers', 'idleBody', 'idleSend', 'total', 'queue', 'stall', 'slow-body'],
      fields: undefined,
    });
    expect(byCode.get('QUEUE_FULL')?.fields).toEqual(['scope', 'bound']);
    expect(byCode.get('QUEUE_FULL')?.details).toEqual(['client', 'origin', 'bytes', 'contexts', 'global']);
    expect(byCode.get('TOO_MANY_REDIRECTS')).toEqual({
      code: 'TOO_MANY_REDIRECTS',
      details: undefined,
      fields: ['count'],
    });
  });

  it('formats codes as CODE/detail', () => {
    expect(formatErrorCode('PROTOCOL', 'malformed-response')).toBe('PROTOCOL/malformed-response');
    expect(formatErrorCode('TOO_LARGE', 'headers')).toBe('TOO_LARGE/headers');
    expect(formatErrorCode('NETWORK', 'dns/notfound')).toBe('NETWORK/dns/notfound');
    const full: ErrorDetailMap['QUEUE_FULL'] = { scope: 'global', bound: 4 };
    expect(formatErrorCode('QUEUE_FULL', full)).toBe('QUEUE_FULL/global');
    expect(formatErrorCode('TOO_MANY_REDIRECTS', { count: 21 })).toBe('TOO_MANY_REDIRECTS');
    expect(formatErrorCode('HTTP_STATUS', { status: 500 })).toBe('HTTP_STATUS');
  });

  it('exposes the H1 limits and methods', () => {
    expect(H1_LIMITS).toEqual({
      maxHeadBytes: 65536,
      maxStatusLineBytes: 8192,
      maxHeaderFields: 100,
      maxInterimResponses: 10,
    });
    const methods: Method[] = ['GET', 'HEAD', 'POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'QUERY'];
    expect(methods).toHaveLength(8);
  });
  it('lists engine errors in formatErrorCode form, wire codes only', () => {
    expect(ENGINE_ERRORS).toContain(formatErrorCode('PROTOCOL', 'malformed-response'));
    expect(ENGINE_ERRORS).toContain(formatErrorCode('QUEUE_FULL', { scope: 'global', bound: 1 }));
    expect(ENGINE_ERRORS).toContain(formatErrorCode('TOO_MANY_REDIRECTS', { count: 21 }));
    expect(ENGINE_ERRORS).not.toContain(formatErrorCode('HTTP_STATUS', { status: 500 }));
    expect(ENGINE_ERRORS).not.toContain(formatErrorCode('PARSE', 'json'));
    expect(new Set(ENGINE_ERRORS).size).toBe(ENGINE_ERRORS.length);
  });

  it('lists the methods at runtime', () => {
    const methods: readonly Method[] = METHODS;
    expect(methods).toContain('QUERY');
  });
});
