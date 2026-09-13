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
    return compareExchangeSnapshots([handle],[expected],[bytes]).then(revisions=>revisions[0]);
}
export function snapshotRevision(record){return record.revision;}
export function snapshotBytes(record){return record.bytes;}

// All keys must live in one named database. Separate connections are supported;
// IndexedDB serializes overlapping read/write transactions across tabs/workers.
export function compareExchangeSnapshots(handles,expected,values){
    if(!Array.isArray(handles) || handles.length<1 || handles.length>128 || expected.length!==handles.length || values.length!==handles.length)return Promise.reject(fail('invalid'));
    const keys=new Set(), copies=[];
    let total=0;
    for(let i=0;i<handles.length;i++){
        const h=handles[i], b=values[i], e=expected[i];
        if(h.closed)return Promise.reject(fail('storage'));
        if(h.db.name!==handles[0].db.name || keys.has(h.scope) || !Number.isSafeInteger(e) || (e!==-1 && e<1) || e>MAX_REVISION
            || (b!==null && (!(b instanceof Uint8Array) || b.byteLength>h.maxBytes)))return Promise.reject(fail('invalid'));
        keys.add(h.scope);total+=b===null?0:b.byteLength;
        if(total>16*1024*1024)return Promise.reject(fail('invalid'));
        copies.push(b===null?null:b.slice());
    }
    return new Promise((resolve,reject)=>{
        let tx,failure;const revisions=new Array(handles.length);
        try{tx=handles[0].db.transaction(STORE,'readwrite',{durability:'strict'});}catch(_){reject(fail('storage'));return;}
        tx.oncomplete=()=>resolve(revisions);
        tx.onabort=()=>reject(failure||fail('storage'));
        tx.onerror=()=>{};
        try{
            const store=tx.objectStore(STORE);
            const countRequest=store.count();
            countRequest.onsuccess=()=>{
            let slots=countRequest.result;
            handles.forEach((h,i)=>{
                const request=store.get(h.scope);
                request.onsuccess=()=>{
                    try{
                        const record=checked(request.result,h);
                        if((record===null?-1:record.revision)!==expected[i])throw fail('storage_conflict');
                        if(record?.revision===MAX_REVISION)throw fail('storage');
                        if(record===null && ++slots>2048)throw fail('storage');
                        revisions[i]=(record?.revision||0)+1;
                        store.put({scope:h.scope,revision:revisions[i],bytes:copies[i]});
                    }catch(error){failure=error?.quaiBrowserError?error:fail('storage');tx.abort();}
                };
            });
            };
        }catch(_){failure=fail('storage');tx.abort();}
    });
}
