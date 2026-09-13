// Browser/worker WebSocket session. No retries, wallet prompts, or write replay.
const sessions = new WeakMap();
const encoder = new TextEncoder();
const fail = (kind) => Object.freeze({ quaiBrowserError: kind });
function state(handle) { const s = sessions.get(handle); if (!s) throw fail('closed'); return s; }
function bytes(text, max) {
    if (typeof text !== 'string') throw fail('invalid');
    if (text.length > max) throw fail('response_size');
    const size = encoder.encode(text).length;
    if (size > max) throw fail('response_size');
    return size;
}
function clearQueue(s, sub) {
    for (const entry of sub.queue) s.queuedBytes -= entry.bytes;
    sub.queue.length = 0;
}
function endSub(s, sub, error) {
    if (sub.delivery) {
        sub.delivery.signal.removeEventListener('abort', sub.delivery.listener);
        sub.delivery = null;
    }
    if (sub.error) return;
    sub.error = error;
    clearQueue(s, sub);
    if (sub.waiter) sub.waiter.finish(error);
}
function close(s, error = fail('closed')) {
    if (s.closed) return;
    s.closed = true;
    if (s.openProvisional) s.openProvisional.signal.removeEventListener('abort', s.openProvisional.listener);
    for (const pending of [...s.pending.values()]) pending.finish(error);
    for (const sub of s.subscriptions.values()) {
        if (sub.provisional) sub.provisional.signal.removeEventListener('abort', sub.provisional.listener);
        endSub(s, sub, error);
    }
    s.socket.onopen = s.socket.onmessage = s.socket.onerror = s.socket.onclose = null;
    try { s.socket.close(); } catch { /* best effort after local invalidation */ }
}
export function closeSocket(handle) { close(state(handle)); }
export function socketOpenState(handle) { return !state(handle).closed; }
export function socketNextId(handle) {
    const s = state(handle);
    if (s.closed) throw fail('closed');
    if (!Number.isSafeInteger(s.nextId)) throw fail('id_exhausted');
    return s.nextId++;
}
export function openSocket(url, timeout, maxBytes, maxRequest, maxFlight, maxSubs, maxQueue, maxQueuedBytes, controller) {
    return new Promise((resolve, reject) => {
        if (controller.signal.aborted) { reject(fail('cancelled')); return; }
        let socket;
        try { socket = new WebSocket(url); } catch { reject(fail('invalid')); return; }
        socket.binaryType = 'arraybuffer';
        const handle = Object.freeze({});
        const s = { socket, closed: false, nextId: 1, pending: new Map(), subscriptions: new Map(),
            pendingSubs: 0, queuedBytes: 0, timeout, maxBytes, maxRequest, maxFlight, maxSubs, maxQueue, maxQueuedBytes };
        sessions.set(handle, s);
        let opening = true;
        const abort = () => { if (opening) finish(fail('cancelled')); else close(s, fail('cancelled')); };
        s.openProvisional = { signal: controller.signal, listener: abort };
        const timer = setTimeout(() => finish(fail('timeout')), timeout);
        function finish(error) {
            if (!opening) return;
            opening = false;
            clearTimeout(timer);
            if (error) { close(s, error); reject(error); } else resolve(handle);
        }
        controller.signal.addEventListener('abort', abort, { once: true });
        socket.onopen = () => finish();
        socket.onerror = socket.onclose = () => { if (opening) finish(fail('closed')); else close(s); };
        socket.onmessage = (event) => {
            try {
                const size = bytes(event.data, s.maxBytes);
                const value = JSON.parse(event.data);
                if (!value || value.jsonrpc !== '2.0' || Array.isArray(value)) throw fail('invalid');
                if (Object.hasOwn(value, 'id')) {
                    if (!Number.isSafeInteger(value.id) || value.id <= 0 || value.id >= s.nextId) throw fail('invalid');
                    const p = s.pending.get(value.id);
                    // Replies to cancelled/timed-out requests never get reassigned.
                    if (!p) return;
                    if (p.kind === 'subscribe' && !Object.hasOwn(value, 'error')) {
                        const id = value.result;
                        if (typeof id !== 'string' || !/^0x[0-9a-fA-F]{1,128}$/.test(id) || s.subscriptions.has(id)) throw fail('invalid');
                        const listener = () => close(s, fail('cancelled'));
                        p.controller.signal.addEventListener('abort', listener, { once: true });
                        s.subscriptions.set(id, { queue: [], waiter: null, error: null, provisional: { signal: p.controller.signal, listener } });
                    }
                    p.finish(null, event.data);
                    return;
                }
                if (value.method !== 'quai_subscription' || !value.params || typeof value.params.subscription !== 'string' || !Object.hasOwn(value.params, 'result')) throw fail('invalid');
                const sub = s.subscriptions.get(value.params.subscription);
                if (!sub || sub.error) return;
                if (sub.waiter) { sub.waiter.finish(null, event.data); return; }
                if (sub.queue.length >= s.maxQueue || size > s.maxQueuedBytes - s.queuedBytes) {
                    endSub(s, sub, fail('lagged')); return;
                }
                sub.queue.push({ text: event.data, bytes: size }); s.queuedBytes += size;
            } catch (error) { close(s, error?.quaiBrowserError ? error : fail('invalid')); }
        };
    });
}
export function socketRequest(handle, id, body, kind, controller) {
    const s = state(handle);
    return new Promise((resolve, reject) => {
        if (s.closed) { reject(fail('closed')); return; }
        if (controller.signal.aborted) { reject(fail('cancelled')); return; }
        let size;
        try { size = bytes(body, s.maxRequest); } catch { reject(fail('request_size')); return; }
        if (size > s.maxRequest * s.maxFlight - s.socket.bufferedAmount) { reject(fail('busy')); return; }
        if (s.pending.size >= s.maxFlight || (kind === 'subscribe' && s.subscriptions.size + s.pendingSubs >= s.maxSubs)) { reject(fail('busy')); return; }
        if (!Number.isSafeInteger(id) || id <= 0 || id >= s.nextId || s.pending.has(id)) { reject(fail('invalid')); return; }
        let settled = false;
        const abort = () => finish(fail('cancelled'));
        const timer = setTimeout(() => finish(fail('timeout')), s.timeout);
        function finish(error, value) {
            if (settled) return;
            settled = true; clearTimeout(timer); controller.signal.removeEventListener('abort', abort);
            s.pending.delete(id); if (kind === 'subscribe') s.pendingSubs--;
            // A cancelled subscribe may already exist remotely under an unknown
            // server ID. Closing bounds that ambiguity instead of leaking it.
            if (error && kind === 'subscribe') close(s, error);
            if (error) reject(error); else resolve(value);
        }
        s.pending.set(id, { finish, kind, controller }); if (kind === 'subscribe') s.pendingSubs++;
        controller.signal.addEventListener('abort', abort, { once: true });
        try { s.socket.send(body); } catch { finish(fail('closed')); close(s); }
    });
}
export function socketNext(handle, id, controller) {
    const s = state(handle);
    return new Promise((resolve, reject) => {
        const sub = s.subscriptions.get(id);
        if (s.closed) { reject(fail('closed')); return; }
        if (!sub) { reject(fail('subscription_closed')); return; }
        if (sub.error) { reject(sub.error); return; }
        if (controller.signal.aborted) { reject(fail('cancelled')); return; }
        if (sub.waiter || sub.delivery) { reject(fail('busy')); return; }
        const deliver = value => {
            const listener = () => endSub(s, sub, fail('lagged'));
            sub.delivery = { signal: controller.signal, listener };
            controller.signal.addEventListener('abort', listener, { once: true });
            resolve(value);
        };
        if (sub.queue.length) { const item = sub.queue.shift(); s.queuedBytes -= item.bytes; deliver(item.text); return; }
        let settled = false;
        const abort = () => finish(fail('cancelled'));
        const timer = setTimeout(() => finish(fail('timeout')), s.timeout);
        function finish(error, value) {
            if (settled) return;
            settled = true; clearTimeout(timer); controller.signal.removeEventListener('abort', abort);
            sub.waiter = null;
            if (error) reject(error); else deliver(value);
        }
        sub.waiter = { finish };
        controller.signal.addEventListener('abort', abort, { once: true });
    });
}
export function forgetSubscription(handle, id) {
    const s = state(handle); const sub = s.subscriptions.get(id);
    if (sub) { endSub(s, sub, fail('subscription_closed')); s.subscriptions.delete(id); }
}
export function dropSubscription(handle, id) {
    const s = state(handle); forgetSubscription(handle, id);
    if (s.closed) return;
    // A best-effort bounded unsubscribe; if cleanup cannot be queued, close the
    // session instead of silently leaving an unbounded remote subscription.
    if (s.pending.size >= s.maxFlight) { close(s); return; }
    try {
        const requestId = socketNextId(handle);
        const body = JSON.stringify({ jsonrpc: '2.0', id: requestId, method: 'quai_unsubscribe', params: [id] });
        socketRequest(handle, requestId, body, 'request', new AbortController())
            .then(raw => { if (JSON.parse(raw).result !== true) close(s); })
            .catch(() => close(s));
    } catch { close(s); }
}

export function acknowledgeSubscription(handle, id) {
    const sub = state(handle).subscriptions.get(id);
    if (!sub) throw fail('subscription_closed');
    if (sub.provisional) {
        sub.provisional.signal.removeEventListener('abort', sub.provisional.listener);
        sub.provisional = null;
    }
}

export function acknowledgeSocket(handle) {
    const s = state(handle);
    if (s.closed) throw fail('closed');
    if (s.openProvisional) {
        s.openProvisional.signal.removeEventListener('abort', s.openProvisional.listener);
        s.openProvisional = null;
    }
}

export function acknowledgeNotification(handle, id) {
    const sub = state(handle).subscriptions.get(id);
    if (!sub || sub.error || !sub.delivery) throw fail('subscription_closed');
    sub.delivery.signal.removeEventListener('abort', sub.delivery.listener);
    sub.delivery = null;
}
