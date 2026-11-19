// Product bridge. One pi-ai provider over the host model socket.
// No faux provider, no credential file, and no name lookup.
//
// The HTTP path is pi-ai's openai-completions client, not a path chosen here.
// `PRODUCT_BASE_URL` is passed as `model.baseUrl`. That client posts
// `/chat/completions` (`openai` SDK `chat.completions.create`) with `stream: true`,
// and `buildURL` concatenates the base and that path, so the socket request is
// `POST /v1/chat/completions`. The SDK then reads the body as SSE. The base URL
// host is not dialed; the only connect is the Unix socket.
import { stream as openAIStream, streamSimple as openAIStreamSimple } from '@earendil-works/pi-ai/api/openai-completions';
import { createModels, createProvider, type Provider } from '@earendil-works/pi-ai/models';
import type { Model, TranscriptContext } from '@earendil-works/pi-ai';
import { connect, type Socket } from 'node:net';
import { forwarders } from './forwarders.ts';
import type { Binding } from './protocol.ts';
import { Runtime } from './runtime.ts';
import { Transport } from './transport.ts';

export const PRODUCT_SOCKET = '/model-proxy/endpoint.sock';
/** Fixed base URL handed to pi-ai. Not a dial target. */
export const PRODUCT_BASE_URL = 'http://127.0.0.1/v1';
export const PRODUCT_PROVIDER = 'switchyard';
export const PRODUCT_MODEL = 'switchyard-product-1';
const KEYLESS = 'unused';

type Head = { status: number; headers: Headers; mode: 'length' | 'chunked' | 'close'; length: number };

function parseHead(buffer: Buffer): { head: Head; rest: Buffer } | null {
  const split = buffer.indexOf('\r\n\r\n');
  if (split < 0) return null;
  const lines = buffer.subarray(0, split).toString('utf8').split('\r\n');
  const status = /^HTTP\/1\.[01] (\d+)/.exec(lines[0] ?? '');
  if (!status) throw new Error('model socket response has no status');
  const headers = new Headers();
  for (const line of lines.slice(1)) {
    const index = line.indexOf(':');
    if (index > 0) headers.append(line.slice(0, index).trim(), line.slice(index + 1).trim());
  }
  const transfer = headers.get('transfer-encoding') ?? '';
  const lengthHeader = headers.get('content-length');
  const mode = transfer.toLowerCase().includes('chunked') ? 'chunked' : lengthHeader == null ? 'close' : 'length';
  return {
    head: { status: Number(status[1]), headers, mode, length: lengthHeader == null ? 0 : Number(lengthHeader) },
    rest: buffer.subarray(split + 4),
  };
}

/** Decode one chunked body. Returns bytes that are complete entity data. */
function takeChunked(state: { buffer: Buffer; done: boolean }): Buffer {
  const out: Buffer[] = [];
  while (!state.done) {
    const lineEnd = state.buffer.indexOf('\r\n');
    if (lineEnd < 0) break;
    const size = Number.parseInt(state.buffer.subarray(0, lineEnd).toString('utf8').split(';')[0] ?? '', 16);
    if (!Number.isFinite(size)) throw new Error('model socket chunk size is invalid');
    if (size === 0) {
      const trailer = state.buffer.indexOf('\r\n\r\n', lineEnd);
      if (trailer < 0) break;
      state.buffer = state.buffer.subarray(trailer + 4);
      state.done = true;
      break;
    }
    const start = lineEnd + 2;
    if (state.buffer.length < start + size + 2) break;
    out.push(state.buffer.subarray(start, start + size));
    state.buffer = state.buffer.subarray(start + size + 2);
  }
  return Buffer.concat(out);
}

function socketFetch(socketPath: string): typeof globalThis.fetch {
  return async (input, init) => {
    const request = new Request(input, init);
    const url = new URL(request.url);
    const body = new Uint8Array(await request.arrayBuffer());
    const lines = [`${request.method} ${url.pathname}${url.search} HTTP/1.1`, `Host: ${url.host}`];
    request.headers.forEach((value, key) => {
      if (key === 'host' || key === 'content-length') return;
      lines.push(`${key}: ${value}`);
    });
    lines.push(`Content-Length: ${body.byteLength}`, 'Connection: close', '', '');
    const payload = Buffer.concat([Buffer.from(lines.join('\r\n')), body]);
    return await new Promise((resolve, reject) => {
      const socket: Socket = connect(socketPath);
      const fail = (error: unknown) => {
        socket.destroy();
        reject(error instanceof Error ? error : new Error(String(error)));
      };
      if (request.signal.aborted) {
        fail(new Error('model socket request aborted'));
        return;
      }
      const onAbort = () => fail(new Error('model socket request aborted'));
      request.signal.addEventListener('abort', onAbort, { once: true });
      let buffer = Buffer.alloc(0);
      let head: Head | null = null;
      let received = 0;
      let settled = false;
      const chunked = { buffer: Buffer.alloc(0), done: false };
      let controller!: ReadableStreamDefaultController<Uint8Array>;
      const stream = new ReadableStream<Uint8Array>({ start(control) { controller = control; } });
      const finish = () => {
        if (settled) return;
        settled = true;
        request.signal.removeEventListener('abort', onAbort);
        controller.close();
      };
      const pushBody = (bytes: Buffer) => {
        if (bytes.length === 0) return;
        controller.enqueue(bytes);
      };
      const consume = () => {
        if (!head) {
          const parsed = parseHead(buffer);
          if (!parsed) return;
          head = parsed.head;
          buffer = Buffer.from(parsed.rest);
          resolve(new Response(stream, { status: head.status, headers: head.headers }));
        }
        if (head.mode === 'chunked') {
          chunked.buffer = Buffer.concat([chunked.buffer, buffer]);
          buffer = Buffer.alloc(0);
          pushBody(takeChunked(chunked));
          if (chunked.done) finish();
          return;
        }
        if (head.mode === 'length') {
          const need = head.length - received;
          const take = buffer.subarray(0, need);
          received += take.length;
          buffer = buffer.subarray(take.length);
          pushBody(take);
          if (received >= head.length) finish();
          return;
        }
        pushBody(buffer);
        buffer = Buffer.alloc(0);
      };
      socket.on('error', fail);
      socket.on('data', chunk => { buffer = Buffer.concat([buffer, chunk]); try { consume(); } catch (error) { fail(error); } });
      socket.on('end', () => {
        if (settled) return;
        if (!head) {
          fail(new Error('model socket response has no header'));
          return;
        }
        if (head.mode === 'close' || (head.mode === 'length' && received >= head.length) || chunked.done) finish();
        else fail(new Error('model socket response ended early'));
      });
      socket.on('connect', () => socket.end(payload));
    });
  };
}

function productModel(): Model<'openai-completions'> {
  return {
    id: PRODUCT_MODEL,
    name: 'Switchyard product socket',
    api: 'openai-completions',
    provider: PRODUCT_PROVIDER,
    baseUrl: PRODUCT_BASE_URL,
    input: ['text'],
    reasoning: false,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 8192,
    maxTokens: 2048,
  };
}

/** Keyless provider. Auth does not read the environment or any file. */
export function productProvider(socketPath: string): Provider {
  const model = productModel();
  const fetch = socketFetch(socketPath);
  const call = (context: TranscriptContext, options: { apiKey?: string; fetch?: typeof fetch; signal?: AbortSignal } | undefined, simple: boolean) => {
    const next = { ...options, apiKey: KEYLESS, fetch };
    return simple
      ? openAIStreamSimple(model, context, next)
      : openAIStream(model, context, next);
  };
  return createProvider({
    id: PRODUCT_PROVIDER,
    name: 'Switchyard',
    auth: {
      apiKey: {
        name: 'keyless local socket',
        resolve: async () => ({ auth: { apiKey: KEYLESS }, source: 'keyless' }),
      },
    },
    models: [model],
    api: {
      stream: (_model, context, options) => call(context, options, false),
      streamSimple: (_model, context, options) => call(context, options, true),
    },
  });
}

const [database, bindingJson, mode] = process.argv.slice(2);
if (process.argv[1]?.endsWith('product-child.ts') && database && bindingJson && mode) {
  if (mode !== 'product') throw new Error('Invalid launcher arguments');
  const binding = JSON.parse(bindingJson) as Binding;
  const models = createModels();
  models.setProvider(productProvider(PRODUCT_SOCKET));
  const transport = new Transport(binding);
  const runtime = new Runtime(database, binding, models, forwarders(binding, transport), {}, {
    provider: PRODUCT_PROVIDER, modelId: PRODUCT_MODEL,
  });
  void runtime.failed.then(error => transport.failFatal(error));
  transport.start(frame => runtime.dispatch(frame));
}
