import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { watchProvider, providerRevision, providerEventsSupported, providerChanged, closeProviderWatch } from '../src/bridge.js';
test('event revisions retain no payload, invalidate transient changes, and remove listeners', () => {
    const provider = new EventEmitter();
    const watch = watchProvider(provider);
    assert.equal(providerEventsSupported(watch),true);
    for (const event of ['chainChanged','accountsChanged','disconnect']) provider.emit(event,{secret:'never retain'});
    assert.equal(providerRevision(watch),3);
    assert.equal(providerChanged(watch,0),true);
    assert.equal(providerChanged(watch,3),false);
    assert.equal(JSON.stringify(watch).includes('never retain'),false);
    closeProviderWatch(watch);
    closeProviderWatch(watch);
    assert.deepEqual(provider.eventNames(),[]);
    assert.equal(providerChanged(watch,3),true);
});
test('partial listener registration is rolled back and counter overflow fails closed', () => {
    const provider = new EventEmitter();
    const on = provider.on.bind(provider);
    provider.on = (event,listener) => { on(event,listener); if(event==='accountsChanged') throw new Error('fixture'); };
    assert.throws(()=>watchProvider(provider));
    assert.deepEqual(provider.eventNames(),[]);
    provider.on = on;
    const watch = watchProvider(provider);
    watch.revision = 0xffffffff-1;
    provider.emit('chainChanged');
    assert.equal(providerChanged(watch,0xffffffff),true);
    closeProviderWatch(watch);
});
test('request-only adapters explicitly report unavailable event monitoring', () => {
    const watch = watchProvider({request:async()=>null});
    assert.equal(providerEventsSupported(watch),false);
    closeProviderWatch(watch);
});
