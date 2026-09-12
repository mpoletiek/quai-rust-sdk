// Browser-only bridge. Exported functions never forward remote message/data into diagnostics.
const encoder = new TextEncoder();
const failure = (kind, code = 0) => Object.freeze({ quaiBrowserError: kind, code });
export function newAbort() { return new AbortController(); }
export function abort(controller) { controller.abort(); }
export function errorKind(error) { return error?.quaiBrowserError || 'transport'; }
export function errorCode(error) { return Number.isSafeInteger(error?.code) ? error.code : 0; }
export function validateProvider(provider) { return provider !== null && typeof provider === 'object' && typeof provider.request === 'function'; }

// A JSON-only, bounded serializer. Accessors/toJSON/cycles/BigInt are rejected.
// Existing memory owned by an injected JS provider is outside the adapter's budget.
export function boundedJson(value, limit) {
    let bytes = 0;
    let nodes = 0;
    const chunks = [];
    const ancestors = new Set();
    function append(text) {
        if (text.length > limit - bytes) throw failure('response_size');
        bytes += encoder.encode(text).byteLength;
        if (bytes > limit) throw failure('response_size');
        chunks.push(text);
    }
    function visit(item, depth) {
        if (++nodes > limit || depth > 64) throw failure('invalid');
        if (item === null) { append('null'); return; }
        switch (typeof item) {
            case 'string':
                if (item.length > limit - bytes) throw failure('response_size');
                append(JSON.stringify(item)); return;
            case 'boolean': append(item ? 'true' : 'false'); return;
            case 'number':
                if (!Number.isFinite(item) || (Number.isInteger(item) && !Number.isSafeInteger(item))) throw failure('invalid');
                append(String(item)); return;
            case 'object': break;
            default: throw failure('invalid');
        }
        if (ancestors.has(item)) throw failure('invalid');
        ancestors.add(item);
        if (Array.isArray(item)) {
            if (item.length > limit - bytes) throw failure('response_size');
            append('[');
            for (let i = 0; i < item.length; i++) {
                if (i) append(',');
                const descriptor = Object.getOwnPropertyDescriptor(item, String(i));
                if (!descriptor || !('value' in descriptor)) throw failure('invalid');
                visit(descriptor.value, depth + 1);
            }
            append(']');
        } else {
            const proto = Object.getPrototypeOf(item);
            if (proto !== Object.prototype && proto !== null) throw failure('invalid');
            append('{');
            let first = true;
            for (const key in item) {
                if (!Object.hasOwn(item, key)) continue;
                const descriptor = Object.getOwnPropertyDescriptor(item, key);
                if (!descriptor || !('value' in descriptor)) throw failure('invalid');
                if (!first) append(',');
                first = false;
                if (key.length > limit - bytes) throw failure('response_size');
                append(JSON.stringify(key)); append(':'); visit(descriptor.value, depth + 1);
            }
            append('}');
        }
        ancestors.delete(item);
    }
    visit(value, 0);
    return chunks.join('');
}

async function boundedOperation(controller, timeout, operation) {
    let timedOut = false;
    let listener;
    const timer = setTimeout(() => { timedOut = true; controller.abort(); }, timeout);
    const cancelled = new Promise((_, reject) => {
        listener = () => reject(failure(timedOut ? 'timeout' : 'cancelled'));
        controller.signal.addEventListener('abort', listener, { once: true });
        if (controller.signal.aborted) listener();
    });
    try { return await Promise.race([operation(), cancelled]); }
    catch (error) {
        if (controller.signal.aborted) throw failure(timedOut ? 'timeout' : 'cancelled');
        if (error?.quaiBrowserError) throw error;
        throw failure('transport');
    } finally {
        clearTimeout(timer);
        controller.signal.removeEventListener('abort', listener);
    }
}

export async function fetchJson(url, body, controller, timeout, maxBytes) {
    return boundedOperation(controller, timeout, async () => {
        const response = await globalThis.fetch(url, {
            method: 'POST', body, headers: { 'Content-Type': 'application/json' },
            mode: 'cors', credentials: 'omit', redirect: 'error', cache: 'no-store',
            referrerPolicy: 'no-referrer', signal: controller.signal,
        });
        if (!response.ok) throw failure('http', response.status);
        if (response.redirected || !response.body) throw failure('invalid');
        const length = response.headers.get('Content-Length');
        if (length !== null && (!/^\d+$/.test(length) || Number(length) > maxBytes)) throw failure('response_size');
        const reader = response.body.getReader();
        const chunks = [];
        let size = 0;
        try {
            for (;;) {
                const { value, done } = await reader.read();
                if (done) break;
                if (!(value instanceof Uint8Array) || value.byteLength > maxBytes - size) throw failure('response_size');
                size += value.byteLength;
                chunks.push(value);
            }
            const bytes = new Uint8Array(size);
            let offset = 0;
            for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
            try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
            catch (_) { throw failure('invalid'); }
        } finally {
            // Cancellation does not await a possibly hostile stream's cancel promise.
            reader.cancel().catch(() => {});
            reader.releaseLock();
        }
    });
}

export async function injectedJson(provider, payload, controller, timeout, maxBytes) {
    return boundedOperation(controller, timeout, async () => {
        let result;
        try { result = await provider.request(JSON.parse(payload)); }
        catch (error) {
            let code = 0;
            try { if (Number.isSafeInteger(error?.code)) code = error.code; } catch (_) {}
            throw failure('provider', code);
        }
        if (controller.signal.aborted) throw failure('cancelled');
        return boundedJson(result, maxBytes);
    });
}

export function randomBytes(length) {
    if (!globalThis.isSecureContext || !globalThis.crypto?.getRandomValues || length > 65536) throw failure('entropy');
    return globalThis.crypto.getRandomValues(new Uint8Array(length));
}
