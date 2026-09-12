import { createInterface } from 'node:readline';
import { jsonlResponse } from './reference.mjs';

// JSONL request: {"id":"optional","operation":"address","input":{...}}
// JSONL response: {"id":"optional","result":...}; errors contain only a code.
// Public test inputs only. Never feed production secrets into this development tool.
for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  if (!line.trim()) continue;
  process.stdout.write(`${JSON.stringify(jsonlResponse(line))}\n`);
}
