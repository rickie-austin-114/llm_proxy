import assert from 'node:assert/strict';
import { request as httpRequest } from 'node:http';

const origin = 'http://127.0.0.1:4011';
const headers = { 'Content-Type': 'application/json' };
const officeOrigin = 'https://pivot.claude.ai';
let checks = 0;
for (const provider of ['openai', 'claude']) {
  const claude = provider === 'claude';
  const base = claude ? '/claude/v1' : '/openai/v1';
  const auth = claude
    ? {
        'x-api-key': 'fixture-claude-key',
        'anthropic-version': '2023-06-01',
        Origin: officeOrigin,
      }
    : { Authorization: 'Bearer fixture-openai-key' };
  let response = await fetch(`${origin}${base}/models`, { headers: auth });
  assert.equal(response.status, 200);
  if (claude) assert.equal(response.headers.get('access-control-allow-origin'), officeOrigin);
  assert.equal((await response.json()).data[0].id, 'fixture-model');
  checks++;

  const path = `${origin}${base}${claude ? '/messages' : '/chat/completions'}`;
  const payload = {
    model: 'fixture-model',
    max_tokens: 8,
    messages: [{ role: 'user', content: 'fixture prompt' }],
  };
  response = await fetch(path, {
    method: 'POST',
    headers: { ...headers, ...auth },
    body: JSON.stringify(payload),
  });
  assert.equal(response.status, 200);
  if (claude) assert.equal(response.headers.get('access-control-allow-origin'), officeOrigin);
  assert.match(await response.text(), /fixture/);
  checks++;

  response = await fetch(path, {
    method: 'POST',
    headers: { ...headers, ...auth },
    body: JSON.stringify({ ...payload, stream: true }),
  });
  assert.equal(response.status, 200);
  if (claude) assert.equal(response.headers.get('access-control-allow-origin'), officeOrigin);
  const reader = response.body.getReader();
  const first = await reader.read();
  assert.equal(first.done, false);
  assert.match(new TextDecoder().decode(first.value), /fixture/);
  let stream = '';
  for (;;) {
    const chunk = await reader.read();
    if (chunk.done) break;
    stream += new TextDecoder().decode(chunk.value);
  }
  assert.match(stream, claude ? /message_stop/ : /\[DONE\]/);
  checks++;

  response = await fetch(path, {
    method: 'POST',
    headers: { ...headers, ...auth },
    body: JSON.stringify({ ...payload, model: 'fixture-rate-limit' }),
  });
  assert.equal(response.status, 429);
  if (claude) {
    assert.equal(response.headers.get('access-control-allow-origin'), officeOrigin);
    assert.match(response.headers.get('access-control-expose-headers'), /retry-after/);
  }
  assert.equal(response.headers.get('retry-after'), '2');
  assert.equal(response.headers.get('x-request-id'), 'fixture-request');
  assert.equal((await response.json()).error.type, 'rate_limit_error');
  checks++;
}
for (const [path, auth] of [
  ['/openai/v1/models', {}],
  ['/claude/v1/models', {}],
  ['/openai/v1/models', { Authorization: 'Bearer fixture-claude-key' }],
  ['/claude/v1/models', { 'x-api-key': 'fixture-openai-key' }],
  ['/openai/v1/models', { Authorization: 'Bearer fixture-local-token' }],
  ['/claude/v1/models', { 'x-api-key': 'fixture-local-token' }],
  [
    '/openai/v1/models',
    { Authorization: 'Bearer fixture-openai-key', 'x-api-key': 'fixture-claude-key' },
  ],
]) {
  const denied = await fetch(`${origin}${path}`, { headers: auth });
  assert.equal(denied.status, 401);
  checks++;
}
for (const path of ['/claude/v1/messages', '/openai/v1/chat/completions']) {
  const response = await fetch(`${origin}${path}`, {
    method: 'OPTIONS',
    headers: {
      Origin: officeOrigin,
      'Access-Control-Request-Method': 'POST',
      'Access-Control-Request-Headers':
        'x-api-key, authorization, content-type, anthropic-version, anthropic-beta, x-future-sdk-header',
    },
  });
  assert.equal(response.status, 204);
  assert.equal(response.headers.get('access-control-allow-origin'), officeOrigin);
  assert.match(response.headers.get('access-control-allow-headers'), /authorization/);
  assert.match(response.headers.get('access-control-allow-headers'), /x-future-sdk-header/);
  assert.match(response.headers.get('vary'), /access-control-request-headers/i);
  assert.equal(response.headers.get('access-control-allow-credentials'), null);
  checks++;
}
for (const browserOrigin of [
  'https://untrusted.invalid',
  'https://other.claude.ai',
  'https://pivot.claude.ai.untrusted.invalid',
]) {
  const response = await fetch(`${origin}/claude/v1/models`, {
    headers: { Origin: browserOrigin, 'x-api-key': 'fixture-claude-key' },
  });
  assert.equal(response.status, 403);
  assert.equal(response.headers.get('access-control-allow-origin'), null);
  checks++;
}
const unauthorizedOffice = await fetch(`${origin}/claude/v1/models`, {
  headers: { Origin: officeOrigin },
});
assert.equal(unauthorizedOffice.status, 401);
assert.equal(unauthorizedOffice.headers.get('access-control-allow-origin'), officeOrigin);
checks++;
const unsupportedPreflight = await fetch(`${origin}/claude/v1/messages`, {
  method: 'OPTIONS',
  headers: { Origin: officeOrigin, 'Access-Control-Request-Method': 'DELETE' },
});
assert.equal(unsupportedPreflight.status, 400);
assert.equal(unsupportedPreflight.headers.get('access-control-allow-origin'), officeOrigin);
checks++;
// Fetch normalizes Host; use the HTTP transport to exercise a forged value.
const badHostStatus = await new Promise((resolve, reject) => {
  const request = httpRequest(
    `${origin}/claude/v1/messages`,
    {
      method: 'OPTIONS',
      headers: {
        Origin: officeOrigin,
        Host: 'untrusted.invalid',
        'Access-Control-Request-Method': 'POST',
      },
    },
    (response) => {
      response.resume();
      resolve(response.statusCode);
    },
  );
  request.on('error', reject);
  request.setTimeout(5000, () => request.destroy(new Error('Fixture request timed out.')));
  request.end();
});
assert.equal(badHostStatus, 400);
checks++;
console.log(`Passed ${checks} checks through the real Rust proxy and local Postman simulation.`);
