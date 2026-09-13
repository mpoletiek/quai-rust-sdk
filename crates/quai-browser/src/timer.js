// Owned timers for bounded read-only polling in windows and dedicated workers.
export function monotonicNow() {
    const value = globalThis.performance?.now();
    if (!Number.isFinite(value) || value < 0) throw new Error('clock unavailable');
    return value;
}
export function newTimer(milliseconds) {
    if (!Number.isInteger(milliseconds) || milliseconds < 1 || milliseconds > 0x7fffffff)
        throw new Error('invalid timer');
    const handle = { id: null, resolve: null, done: false, promise: null };
    handle.promise = new Promise(resolve => { handle.resolve = resolve; });
    handle.id = globalThis.setTimeout(() => {
        handle.id = null;
        handle.done = true;
        const resolve = handle.resolve;
        handle.resolve = null;
        resolve(true);
    }, milliseconds);
    return handle;
}
export function waitTimer(handle) { return handle.promise; }
export function closeTimer(handle) {
    if (handle.done) return;
    handle.done = true;
    if (handle.id !== null) globalThis.clearTimeout(handle.id);
    handle.id = null;
    const resolve = handle.resolve;
    handle.resolve = null;
    resolve(false); // Cancellation never creates an unhandled rejected promise.
}
