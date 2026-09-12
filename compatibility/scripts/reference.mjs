import * as q from 'quais';

// Deliberately allowlisted, deterministic operations. No signing, wallet import,
// filesystem access, provider discovery, or arbitrary module/function execution.
export function evaluate({ operation, input }) {
  switch (operation) {
    case 'address': {
      const checksum = q.getAddress(input.address);
      return { checksum, zone: q.getZoneForAddress(checksum), ledger: q.isQiAddress(checksum) ? 'qi' : 'quai' };
    }
    case 'hashMessage': return q.hashMessage(input.encoding === 'hex' ? q.getBytes(input.message) : input.message);
    case 'getCreateAddress': return q.getCreateAddress({ from: input.from, nonce: BigInt(input.nonce), data: input.data });
    case 'keccak256': return q.keccak256(input.data);
    case 'parseUnits': return q.parseUnits(input.value, input.decimals).toString();
    case 'formatUnits': return q.formatUnits(BigInt(input.value), input.decimals);
    default: throw Object.assign(new Error('Unsupported reference operation'), { code: 'UNSUPPORTED_OPERATION' });
  }
}

export function outcome(request) {
  try { return evaluate(request); }
  catch (error) { return { error: { code: error.code ?? error.name } }; }
}

export function jsonlResponse(line) {
  try {
    if (line.length > 1048576) throw new Error('oversized input');
    const request = JSON.parse(line);
    if (!request || typeof request !== 'object' || Array.isArray(request)) throw new Error('invalid request');
    return { ...(request.id === undefined ? {} : { id: request.id }), result: outcome(request) };
  } catch {
    return { result: { error: { code: 'INVALID_REQUEST' } } };
  }
}

export function routing(input) {
  // Explicit shards avoid live getRunningLocations discovery entirely.
  const provider = new q.JsonRpcProvider(input.url, undefined, {
    usePathing: input.usePathing,
    shards: [input.shard],
    ...(input.shardPaths ? { shardPaths: input.shardPaths } : {}),
  });
  try { return provider._getConnection(input.shard).url; }
  finally { provider.destroy(); }
}
