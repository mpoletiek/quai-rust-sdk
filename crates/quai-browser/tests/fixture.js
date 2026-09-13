export function fixtureProvider(mode) {
    const calls = [];
    let pending;
    const listeners = new Map();
    return {
        calls,
        on(event, fn) { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn); },
        removeListener(event, fn) { listeners.get(event)?.delete(fn); },
        emit(event) { for (const fn of listeners.get(event) || []) fn(); },
        listenerCount() { return [...listeners.values()].reduce((n, set) => n + set.size, 0); },
        transaction: null,
        signature: "0x" + "11".repeat(64) + "1b",
        finish(value) { pending?.(value); },
        async request(payload) {
            calls.push(payload);
            if (mode === 'hang') return new Promise(resolve => { pending = resolve; });
            if (mode === 'denied') throw { code: 4001, message: 'SECRET_PROVIDER_DATA', data: { secret: true } };
            if (payload.method === 'quai_chainId') return mode === 'wrong_chain' ? '0x9' : '0x3a98';
            if (payload.method === 'quai_requestAccounts' || payload.method === 'quai_accounts') return mode === 'unavailable' ? [] : ['0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32'];
            if (payload.method === 'personal_sign' && mode === 'change_during_sign') this.emit('accountsChanged');
            if (payload.method === 'quai_signTypedData_v4' && mode === 'change_during_sign') this.emit('chainChanged');
            if (payload.method === 'quai_getTransactionByHash') return this.transaction;
            if (payload.method === 'quai_sendTransaction') {
                if (mode === 'wallet_context_change') this.emit('accountsChanged');
                if (mode === 'wallet_denied') throw {code:4001,message:'SECRET_PROVIDER_DATA'};
                if (mode === 'wallet_unsupported') throw {code:4200,message:'SECRET_PROVIDER_DATA'};
                if (mode === 'wallet_hang') return new Promise(resolve => {pending=resolve;});
                return this.signature;
            }
            if (payload.method === 'quai_sendRawTransaction') {
                if (mode === 'change_during_send') this.emit('disconnect');
                if (mode === 'reject_send') throw {code:4200,message:'SECRET_PROVIDER_DATA'};
                if (mode === 'hang_send') return new Promise(resolve => {pending=resolve;});
                if (mode === 'wrong_ack') return '0x'+'00'.repeat(32);
                return this.signature;
            }
            if (payload.method === 'quai_signTransaction') {
                if (mode === 'change_during_sign') this.emit('chainChanged');
                if (mode === 'unsupported_sign') throw {code:4200,message:'SECRET_PROVIDER_DATA'};
                if (mode === 'deny_sign') throw {code:4001,message:'SECRET_PROVIDER_DATA'};
                if (mode === 'hang_sign') return new Promise(resolve => {pending=resolve;});
                return this.signature;
            }
            if (payload.method === 'personal_sign') return mode === 'unicode_signature' ? '0x' + '11'.repeat(63) + 'éé' : this.signature;
            if (payload.method === 'quai_signTypedData_v4') return this.signature;
            if (mode === 'oversize') return 'x'.repeat(10000);
            if (mode === 'accessor') return Object.defineProperty({}, 'secret', {enumerable:true,get(){throw new Error('SECRET_GETTER')}});
            return '0x17';
        }
    };
}
export function callCount(provider) { return provider.calls.length; }
export function callJson(provider, index) { return JSON.stringify(provider.calls[index]); }
export function finishProvider(provider) { provider.finish('0x3a98'); }

export function setSignature(provider, signature) { provider.signature = signature; }

export function listenerCount(provider) { return provider.listenerCount(); }
export function emitContextChange(provider, event) { provider.emit(event); }

export function setTransaction(provider, json) { provider.transaction = JSON.parse(json); }
