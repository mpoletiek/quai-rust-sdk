// General resources use their own policy; these functions never submit JSON-RPC implicitly.
const fail = kind => Object.freeze({ quaiBrowserError: kind });
export async function resourceFetch(url, method, headersJson, body, controller, maxBytes) {
    let reader;
    try {
        const headers = JSON.parse(headersJson);
        // Browsers negotiate compression themselves and forbid these header overrides.
        delete headers['accept-encoding'];
        const response = await globalThis.fetch(url, {
            method, headers, body: body ?? undefined,
            mode: 'cors', credentials: 'omit', redirect: 'manual', cache: 'no-store',
            referrerPolicy: 'no-referrer', signal: controller.signal,
        });
        // Manual redirects are opaque in the Fetch platform, including same-origin.
        // Exposing Location or following them without a bound is not possible here.
        if (response.type === 'opaqueredirect' || response.type === 'opaque' || response.status === 0)
            throw fail('unsupported');
        const collected = Object.create(null);
        let headerBytes = 0, count = 0;
        for (const [key, value] of response.headers) {
            headerBytes += key.length + value.length;
            if (++count > 128 || headerBytes > 16384) throw fail('response_size');
            collected[key] = value;
        }
        const length = response.headers.get('content-length');
        if (method !== 'HEAD' && length !== null && (!/^\d+$/.test(length) || Number(length) > maxBytes))
            throw fail('response_size');
        const chunks = []; let size = 0;
        if (response.body) {
            reader = response.body.getReader();
            for (;;) {
                const {value, done} = await reader.read();
                if (done) break;
                if (!(value instanceof Uint8Array) || value.byteLength > maxBytes-size) throw fail('response_size');
                chunks.push(value); size += value.byteLength;
            }
        }
        const bytes = new Uint8Array(size); let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        return {meta: JSON.stringify({status: response.status, message: response.statusText, headers: collected}), bytes};
    } catch (error) {
        if (error?.quaiBrowserError) throw error;
        throw fail(controller.signal.aborted ? 'cancelled' : 'transport');
    } finally {
        if (reader) {
            try { await reader.cancel(); } catch (_) { /* preserve the primary result */ }
            reader.releaseLock();
        }
    }
}
export function resourceMeta(result) { return result.meta; }
export function resourceBytes(result) { return result.bytes; }
