import { test } from 'node:test';
import assert from 'node:assert/strict';
import { openSocket, acknowledgeNotification, acknowledgeSocket, closeSocket, socketNextId, socketRequest, socketNext, socketOpenState, acknowledgeSubscription, dropSubscription } from '../src/socket.js';
class MockSocket {
    static instances = []; static autoOpen = true;
    constructor(url) { this.url=url;this.sent=[];this.bufferedAmount=0;this.closed=false;MockSocket.instances.push(this);if(MockSocket.autoOpen)queueMicrotask(()=>this.onopen?.()); }
    send(body) { if(this.closed)throw new Error('closed');this.sent.push(JSON.parse(body)); }
    close() { this.closed=true;this.onclose?.(); }
    reply(value) { this.onmessage?.({data: typeof value==='string'?value:JSON.stringify(value)}); }
}
globalThis.WebSocket=MockSocket;
const failure = kind => error => error?.quaiBrowserError===kind;
const tick = () => new Promise(resolve=>setTimeout(resolve,0));
async function open(t,{timeout=100,maxBytes=4096,maxRequest=4096,maxFlight=4,maxSubs=4,maxQueue=4,maxQueuedBytes=8192}={}) {
    const handle=await openSocket('ws://fixture.test/exact?token=PUBLIC',timeout,maxBytes,maxRequest,maxFlight,maxSubs,maxQueue,maxQueuedBytes,new AbortController());
    acknowledgeSocket(handle);t.after(()=>closeSocket(handle));return {handle,socket:MockSocket.instances.at(-1)};
}
function request(handle,method='quai_chainId',params=[],controller=new AbortController()) {
    const id=socketNextId(handle);const body=JSON.stringify({jsonrpc:'2.0',id,method,params});
    return {id,controller,promise:socketRequest(handle,id,body,method==='quai_subscribe'?'subscribe':'request',controller)};
}
const notification = (id,value) => ({jsonrpc:'2.0',method:'quai_subscription',params:{subscription:id,result:value}});
test('responses correlate independently; cancelled late replies never satisfy another request',async t=>{
    const {handle,socket}=await open(t);
    const a=request(handle), b=request(handle);
    socket.reply({jsonrpc:'2.0',id:b.id,result:null});socket.reply({jsonrpc:'2.0',id:a.id,result:'0x9'});
    assert.equal(JSON.parse(await a.promise).result,'0x9');assert.equal(JSON.parse(await b.promise).result,null);
    const cancelled=request(handle), pending=request(handle);
    const rejection=assert.rejects(cancelled.promise,failure('cancelled'));cancelled.controller.abort();await rejection;
    socket.reply({jsonrpc:'2.0',id:cancelled.id,result:'ignored'});
    socket.reply({jsonrpc:'2.0',id:pending.id,result:'kept'});
    assert.equal(JSON.parse(await pending.promise).result,'kept');assert.equal(socket.sent.length,4);
});
test('capacity, output buffering and request size fail before another send',async t=>{
    const {handle,socket}=await open(t,{maxFlight:1,maxRequest:128});
    const a=request(handle);await assert.rejects(request(handle).promise,failure('busy'));
    socket.reply({jsonrpc:'2.0',id:a.id,result:true});await a.promise;
    socket.bufferedAmount=128;await assert.rejects(request(handle).promise,failure('busy'));socket.bufferedAmount=0;
    await assert.rejects(request(handle,'quai_call',['x'.repeat(200)]).promise,failure('request_size'));
    assert.equal(socket.sent.length,1);assert.equal(socketOpenState(handle),true);
});
test('subscribe registers before immediate notification and acknowledgment survives abort cleanup',async t=>{
    const {handle,socket}=await open(t);
    const sub=request(handle,'quai_subscribe',['newHeads']);
    socket.reply({jsonrpc:'2.0',id:sub.id,result:'0x1'});socket.reply(notification('0x1',{number:'0x10'}));
    await sub.promise;acknowledgeSubscription(handle,'0x1');sub.controller.abort();
    assert.equal(socketOpenState(handle),true);
    assert.equal(JSON.parse(await socketNext(handle,'0x1',new AbortController())).params.result.number,'0x10');
    dropSubscription(handle,'0x1');const cleanup=socket.sent.at(-1);assert.equal(cleanup.method,'quai_unsubscribe');
    socket.reply({jsonrpc:'2.0',id:cleanup.id,result:true});await tick();assert.equal(socketOpenState(handle),true);
});
test('cancelled or timed-out subscribe closes the session, including after reply before ownership acknowledgment',async t=>{
    for(const afterReply of [false,true]) {
        const {handle,socket}=await open(t);
        const sub=request(handle,'quai_subscribe',['newHeads']);
        if(afterReply){socket.reply({jsonrpc:'2.0',id:sub.id,result:'0x1'});await sub.promise;sub.controller.abort();}
        else {const rejection=assert.rejects(sub.promise,failure('cancelled'));sub.controller.abort();await rejection;}
        assert.equal(socket.closed,true);assert.equal(socketOpenState(handle),false);
    }
    const {handle}=await open(t,{timeout:10});
    await assert.rejects(request(handle,'quai_subscribe',['newHeads']).promise,failure('timeout'));
    assert.equal(socketOpenState(handle),false);
});
test('notification overflow poisons that stream and releases its queued-byte budget',async t=>{
    const {handle,socket}=await open(t,{maxQueue:1,maxQueuedBytes:200});
    for(const id of ['0x1','0x2']) {const sub=request(handle,'quai_subscribe',['newHeads']);socket.reply({jsonrpc:'2.0',id:sub.id,result:id});await sub.promise;acknowledgeSubscription(handle,id);}
    socket.reply(notification('0x1',1));socket.reply(notification('0x1',2));
    await assert.rejects(socketNext(handle,'0x1',new AbortController()),failure('lagged'));
    socket.reply(notification('0x2',3));assert.equal(JSON.parse(await socketNext(handle,'0x2',new AbortController())).params.result,3);
    assert.equal(socketOpenState(handle),true);
});
test('waiting subscription cancellation and quiet timeout release waiter capacity',async t=>{
    const {handle,socket}=await open(t,{timeout:15});const sub=request(handle,'quai_subscribe',['newHeads']);
    socket.reply({jsonrpc:'2.0',id:sub.id,result:'0x1'});await sub.promise;acknowledgeSubscription(handle,'0x1');
    const controller=new AbortController();const wait=socketNext(handle,'0x1',controller);
    await assert.rejects(socketNext(handle,'0x1',new AbortController()),failure('busy'));
    const rejection=assert.rejects(wait,failure('cancelled'));controller.abort();await rejection;
    await assert.rejects(socketNext(handle,'0x1',new AbortController()),failure('timeout'));
    socket.reply(notification('0x1',null));assert.equal(JSON.parse(await socketNext(handle,'0x1',new AbortController())).params.result,null);
});
test('malformed, future-ID, binary and oversized frames close and fail all pending operations',async t=>{
    for(const value of ['{',JSON.stringify({jsonrpc:'2.0',id:999,result:1}),new ArrayBuffer(2),'x'.repeat(257)]) {
        const {handle,socket}=await open(t,{maxBytes:256});const a=request(handle),b=request(handle);
        const first=assert.rejects(a.promise),second=assert.rejects(b.promise);
        socket.onmessage({data:value});await Promise.all([first,second]);assert.equal(socket.closed,true);
        assert.equal(socket.onmessage,null);assert.equal(socket.sent.length,2);
    }
});
test('duplicate subscription IDs and cleanup rejection close instead of retaining orphaned streams',async t=>{
    const {handle,socket}=await open(t);const first=request(handle,'quai_subscribe',['newHeads']);
    socket.reply({jsonrpc:'2.0',id:first.id,result:'0x1'});await first.promise;acknowledgeSubscription(handle,'0x1');
    const second=request(handle,'quai_subscribe',['newHeads']);const rejection=assert.rejects(second.promise,failure('invalid'));
    socket.reply({jsonrpc:'2.0',id:second.id,result:'0x1'});await rejection;assert.equal(socket.closed,true);
    const another=await open(t);const sub=request(another.handle,'quai_subscribe',['newHeads']);
    another.socket.reply({jsonrpc:'2.0',id:sub.id,result:'0x1'});await sub.promise;acknowledgeSubscription(another.handle,'0x1');
    dropSubscription(another.handle,'0x1');another.socket.reply({jsonrpc:'2.0',id:another.socket.sent.at(-1).id,result:false});await tick();assert.equal(another.socket.closed,true);
});
test('unsubscribe cleanup cannot exceed the request budget or leave a saturated session',async t=>{
    for(const saturated of [false,true]) {
        const {handle,socket}=await open(t,{maxRequest:128,maxFlight:1});const sub=request(handle,'quai_subscribe',['newHeads']);
        const id=saturated?'0x1':'0x'+'a'.repeat(128);socket.reply({jsonrpc:'2.0',id:sub.id,result:id});await sub.promise;acknowledgeSubscription(handle,id);
        const pending=saturated?request(handle):null;
        const rejected=pending?assert.rejects(pending.promise):Promise.resolve();
        dropSubscription(handle,id);await tick();await rejected;assert.equal(socket.closed,true);
    }
});
test('opening cancellation and timeout clean up the socket without replay',async()=>{
    MockSocket.autoOpen=false;
    try {
        const controller=new AbortController();const opening=openSocket('ws://fixture',50,1024,1024,2,2,2,2048,controller);
        const cancelled=assert.rejects(opening,failure('cancelled'));controller.abort();await cancelled;assert.equal(MockSocket.instances.at(-1).closed,true);
        await assert.rejects(openSocket('ws://fixture',10,1024,1024,2,2,2,2048,new AbortController()),failure('timeout'));
        assert.equal(MockSocket.instances.at(-1).closed,true);
    } finally {MockSocket.autoOpen=true;}
});

test('opened but unclaimed connection is closed when its owner is cancelled before resuming',async()=>{
    const controller=new AbortController();
    const handle=await openSocket('ws://fixture',50,1024,1024,2,2,2,2048,controller);
    assert.equal(socketOpenState(handle),true);controller.abort();assert.equal(socketOpenState(handle),false);
    assert.equal(MockSocket.instances.at(-1).closed,true);
});

test('cancelling after notification delivery marks lag instead of silently skipping it',async t=>{
    for(const acknowledge of [false,true]) {
        const {handle,socket}=await open(t);const sub=request(handle,'quai_subscribe',['newHeads']);
        socket.reply({jsonrpc:'2.0',id:sub.id,result:'0x1'});await sub.promise;acknowledgeSubscription(handle,'0x1');
        const controller=new AbortController();const pending=socketNext(handle,'0x1',controller);
        socket.reply(notification('0x1',1));await pending;
        if(acknowledge)acknowledgeNotification(handle,'0x1');
        controller.abort();
        if(!acknowledge)await assert.rejects(socketNext(handle,'0x1',new AbortController()),failure('lagged'));
        else {socket.reply(notification('0x1',2));assert.equal(JSON.parse(await socketNext(handle,'0x1',new AbortController())).params.result,2);}
    }
});
