"""Small bounded RFC6455 loopback fixture; no third-party server dependency."""
import base64
import hashlib
import json
import struct


def exact(stream, count):
    value = stream.read(count)
    if len(value) != count:
        raise EOFError
    return value


def send(handler, payload, opcode=1):
    if not isinstance(payload, bytes):
        payload = json.dumps(payload).encode()
    if len(payload) > 65536:
        raise ValueError('fixture response bound')
    prefix = bytes([0x80 | opcode])
    if len(payload) < 126:
        prefix += bytes([len(payload)])
    elif len(payload) < 65536:
        prefix += bytes([126]) + struct.pack('!H', len(payload))
    else:
        prefix += bytes([127]) + struct.pack('!Q', len(payload))
    handler.wfile.write(prefix + payload)
    handler.wfile.flush()


def receive(handler):
    first, second = exact(handler.rfile, 2)
    if first & 0x70 or not first & 0x80 or not second & 0x80:
        raise ValueError('fixture requires uncompressed final masked frames')
    size = second & 127
    if size == 126:
        size = struct.unpack('!H', exact(handler.rfile, 2))[0]
    elif size == 127:
        size = struct.unpack('!Q', exact(handler.rfile, 8))[0]
    if size > 65536:
        raise ValueError('fixture request bound')
    mask = exact(handler.rfile, 4)
    body = exact(handler.rfile, size)
    return first & 15, bytes(v ^ mask[i % 4] for i, v in enumerate(body))


def serve(handler):
    key = handler.headers.get('Sec-WebSocket-Key', '')
    try:
        if (handler.headers.get('Upgrade', '').lower() != 'websocket'
                or len(base64.b64decode(key, validate=True)) != 16):
            raise ValueError('upgrade')
    except ValueError:
        handler.send_error(400)
        return
    accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
    handler.send_response(101)
    handler.send_header('Upgrade', 'websocket')
    handler.send_header('Connection', 'Upgrade')
    handler.send_header('Sec-WebSocket-Accept', accept)
    handler.end_headers()
    handler.close_connection = True
    handler.connection.settimeout(5)
    subscriptions = 0
    try:
        while True:
            opcode, body = receive(handler)
            if opcode == 8:
                send(handler, b'', 8)
                return
            if opcode == 9:
                send(handler, body, 10)
                continue
            if opcode != 1:
                return
            request = json.loads(body)
            method = request.get('method')
            response = {'jsonrpc': '2.0', 'id': request['id'], 'result': '0x3a98'}
            if method == 'fixture_never':
                continue
            if method == 'fixture_close':
                send(handler, b'', 8)
                return
            if handler.path == '/socket/wrong-id':
                response['id'] += 1
            elif handler.path == '/socket/duplicate-id':
                send(handler, ('{"jsonrpc":"2.0","id":%d,"id":%d,"result":null}' % (request['id'], request['id'])).encode())
                continue
            elif handler.path == '/socket/oversize':
                send(handler, b'x' * 1025)
                continue
            elif handler.path == '/socket/binary':
                send(handler, b'\x00\x01', 2)
                continue
            elif handler.path == '/socket/remote':
                response.pop('result')
                response['error'] = {'code': -32000, 'message': 'public fixture error'}
            if method == 'quai_unsubscribe':
                response['result'] = True
            if method == 'quai_subscribe':
                subscriptions += 1
                response['result'] = hex(subscriptions)
                send(handler, response)
                count = 8 if request['params'][0] == 'burst' else 1
                if request['params'][0] == 'quiet':
                    count = 0
                for n in range(count):
                    if handler.path == '/socket/duplicate-notification':
                        send(handler, ('{"jsonrpc":"2.0","method":"quai_subscription","params":{"subscription":"0x1","subscription":"0x1","result":null}}').encode())
                    else:
                        send(handler, {'jsonrpc': '2.0', 'method': 'quai_subscription', 'params': {'subscription': response['result'], 'result': {'number': hex(16+n)}}})
                continue
            send(handler, response)
    except (EOFError, OSError, ValueError, KeyError):
        return
