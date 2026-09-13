// Atomic, bounded IndexedDB snapshots. Values are public state or caller-encrypted
// envelopes; this adapter never interprets keys or silently uses localStorage.
const fail = kind => Object.freeze({quaiBrowserError:kind});
const STORE='quai_snapshots';
const MAX_REVISION=Number.MAX_SAFE_INTEGER;
export function openSnapshotStore(name,scope,maxBytes) {
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(name) || typeof scope!=='string' || scope.length>256 || !Number.isSafeInteger(maxBytes) || maxBytes<1 || maxBytes>16*1024*1024) throw fail('invalid');
    return new Promise((resolve,reject)=>{
        const request=globalThis.indexedDB.open(`quai-sdk:${name}`,1);
        let settled=false;
        const refuse=()=>{if(!settled){settled=true;reject(fail('storage'));}};
        request.onblocked=refuse;
        request.onerror=refuse;
        request.onupgradeneeded=event=>{
            if (settled || event.oldVersion!==0 || request.result.objectStoreNames.length!==0) {request.transaction.abort();return;}
            request.result.createObjectStore(STORE,{keyPath:'scope'});
        };
        request.onsuccess=()=>{
            const db=request.result;
            if(settled){db.close();return;}
            if(db.objectStoreNames.length!==1 || !db.objectStoreNames.contains(STORE)){db.close();refuse();return;}
            const store=db.transaction(STORE,'readonly').objectStore(STORE);
            if(store.keyPath!=='scope' || store.autoIncrement || store.indexNames.length!==0){db.close();refuse();return;}
            const handle={db,scope,maxBytes,closed:false};
            db.onversionchange=()=>closeSnapshotStore(handle);
            settled=true;resolve(handle);
        };
    });
}
export function closeSnapshotStore(handle){if(!handle.closed){handle.closed=true;handle.db.close();}}
function checked(record,handle){
    if(record===undefined)return null;
    if(!record || record.scope!==handle.scope || !Number.isSafeInteger(record.revision) || record.revision<1 || record.revision>MAX_REVISION
       || (record.bytes!==null && (!(record.bytes instanceof Uint8Array) || record.bytes.byteLength>handle.maxBytes)))throw fail('storage');
    return record;
}
function operation(handle,mode,callback){
    if(handle.closed) return Promise.reject(fail('storage'));
    return new Promise((resolve,reject)=>{
        let result, failure;
        let tx;
        try {tx=handle.db.transaction(STORE,mode,{durability:'strict'});}catch(_){reject(fail('storage'));return;}
        tx.oncomplete=()=>resolve(result);
        tx.onabort=()=>reject(failure||fail('storage'));
        tx.onerror=()=>{};
        try {
            const store=tx.objectStore(STORE), request=store.get(handle.scope);
            request.onsuccess=()=>{try{result=callback(store,checked(request.result,handle));}catch(error){failure=error?.quaiBrowserError?error:fail('storage');tx.abort();}};
        }catch(_){failure=fail('storage');tx.abort();}
    });
}
export function readSnapshot(handle){return operation(handle,'readonly',(_,record)=>record);}
export function compareExchangeSnapshot(handle,expected,bytes){
    if(!Number.isSafeInteger(expected) || expected < -1 || expected > MAX_REVISION || (bytes!==null && (!(bytes instanceof Uint8Array) || bytes.byteLength>handle.maxBytes)))return Promise.reject(fail('invalid'));
    // Copy before waiting for a transaction so caller mutation cannot change its payload.
    const copy=bytes===null?null:bytes.slice();
    return operation(handle,'readwrite',(store,record)=>{
        if((record===null?-1:record.revision)!==expected)throw fail('storage_conflict');
        if(record?.revision===MAX_REVISION)throw fail('storage');
        const revision=(record?.revision||0)+1;
        store.put({scope:handle.scope,revision,bytes:copy});
        return revision;
    });
}
export function snapshotRevision(record){return record.revision;}
export function snapshotBytes(record){return record.bytes;}
