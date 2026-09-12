export function fixtureProvider(mode) {
    const calls = [];
    let pending;
    return {
        calls,
        signature: "0x" + "11".repeat(64) + "1b",
        finish(value) { pending?.(value); },
        async request(payload) {
            calls.push(payload);
            if (mode === 'hang') return new Promise(resolve => { pending = resolve; });
            if (mode === 'denied') throw { code: 4001, message: 'SECRET_PROVIDER_DATA', data: { secret: true } };
            if (payload.method === 'quai_chainId') return mode === 'wrong_chain' ? '0x9' : '0x3a98';
            if (payload.method === 'quai_requestAccounts' || payload.method === 'quai_accounts') return mode === 'unavailable' ? [] : ['0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32'];
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
