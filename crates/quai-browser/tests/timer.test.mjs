import test from 'node:test';
import assert from 'node:assert/strict';
import {newTimer,waitTimer,closeTimer,monotonicNow} from '../src/timer.js';
test('timer bounds reject before allocating a callback',()=>{
    for(const value of [0,-1,0.5,NaN,Infinity,0x80000000,'1',null])assert.throws(()=>newTimer(value));
});
test('owned cancellation clears once and settles without rejection',async()=>{
    const before=monotonicNow();const timer=newTimer(60_000);
    closeTimer(timer);closeTimer(timer);
    assert.equal(await waitTimer(timer),false);
    assert.equal(timer.id,null);assert.equal(timer.resolve,null);assert.equal(timer.done,true);
    assert.ok(monotonicNow()>=before);
});
test('completed timers release callback state and tolerate later close',async()=>{
    const timer=newTimer(1);
    assert.equal(await waitTimer(timer),true);
    assert.equal(timer.id,null);assert.equal(timer.resolve,null);assert.equal(timer.done,true);
    closeTimer(timer);assert.equal(await waitTimer(timer),true);
});
