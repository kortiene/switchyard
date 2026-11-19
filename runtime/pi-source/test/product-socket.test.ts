// Credential-free product provider against a local stub on a Unix socket.
// No live provider, no DNS, and no credential file.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { createModels } from '@earendil-works/pi-ai/models';
import { PRODUCT_MODEL, PRODUCT_PROVIDER, productProvider } from '../src/product-child.ts';

const sse = [
  'data: {"id":"stub","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"stub-done"},"finish_reason":null}]}',
  '',
  'data: {"id":"stub","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}',
  '',
  'data: [DONE]',
  '',
].join('\n');

test('product provider drives a local stub through the socket without credentials', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'sy-product-'));
  const socketPath = join(directory, 'endpoint.sock');
  const secret = 'sk-do-not-send';
  const previousKey = process.env.OPENAI_API_KEY;
  const previousBase = process.env.OPENAI_BASE_URL;
  process.env.OPENAI_API_KEY = secret;
  process.env.OPENAI_BASE_URL = 'http://model.example.invalid/v1';
  let raw = '';
  const server = createServer(socket => {
    const chunks: Buffer[] = [];
    socket.on('data', chunk => chunks.push(chunk));
    socket.on('end', () => {
      raw = Buffer.concat(chunks).toString('utf8');
      const body = Buffer.from(sse);
      socket.end(`HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: ${body.length}\r\nConnection: close\r\n\r\n${sse}`);
    });
  });
  await new Promise<void>(resolve => server.listen(socketPath, resolve));
  try {
    const models = createModels();
    const provider = productProvider(socketPath);
    models.setProvider(provider);
    assert.equal(provider.id, PRODUCT_PROVIDER);
    assert.notEqual(provider.id, 'faux');
    const model = models.getModel(PRODUCT_PROVIDER, PRODUCT_MODEL);
    assert.ok(model);
    const prompt = 'switchyard.work-brief.v1 stub objective';
    const message = await models.completeSimple(model, {
      messages: [{ role: 'user', content: prompt, timestamp: 1 }],
    });
    assert.equal(message.stopReason, 'stop');
    const text = message.content.find(part => part.type === 'text');
    assert.equal(text && text.type === 'text' ? text.text : '', 'stub-done');
    assert.match(raw, /^POST \/v1\/chat\/completions HTTP\/1\.1/);
    assert.match(raw, /"stream"\s*:\s*true/);
    assert.match(raw, /authorization: Bearer unused/i);
    assert.equal(raw.includes(secret), false);
    assert.equal(raw.includes('model.example.invalid'), false);
    assert.equal(raw.includes(prompt), true);
  } finally {
    if (previousKey === undefined) delete process.env.OPENAI_API_KEY;
    else process.env.OPENAI_API_KEY = previousKey;
    if (previousBase === undefined) delete process.env.OPENAI_BASE_URL;
    else process.env.OPENAI_BASE_URL = previousBase;
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  }
});
