// Locally authored transport fixtures, not a complete provider emulator.
const http = require('node:http');

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://fixture.invalid');
  const claude = request.headers['x-api-key'] === 'fixture-claude-key';
  const openai = request.headers.authorization === 'Bearer fixture-openai-key';
  const json = (status, value, headers = {}) => {
    response.writeHead(status, { 'Content-Type': 'application/json', ...headers });
    response.end(JSON.stringify(value));
  };
  if ((!claude && !openai) || (claude && openai)) {
    return json(401, { error: { message: 'Fixture expected the saved provider API key.' } });
  }
  if (claude && request.headers['anthropic-version'] !== '2023-06-01') {
    return json(400, { error: { message: 'Missing fixture API version.' } });
  }

  // @endpoint GET /v1/models
  if (request.method === 'GET' && url.pathname === '/v1/models') {
    return json(200, {
      object: 'list',
      data: [{ id: 'fixture-model', type: 'model', object: 'model' }],
      has_more: false,
    });
  }
  // @endpoint POST /v1/chat/completions
  // @endpoint POST /v1/messages
  const expectedPath = claude ? '/v1/messages' : '/v1/chat/completions';
  if (request.method !== 'POST' || url.pathname !== expectedPath) {
    return json(501, { error: { message: 'Route outside this fixture contract.' } });
  }
  let body;
  try {
    if (request.body !== undefined) {
      body = typeof request.body === 'string' ? JSON.parse(request.body) : request.body;
    } else {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      body = JSON.parse(Buffer.concat(chunks).toString());
    }
  } catch {
    return json(400, { error: { message: 'Expected JSON.' } });
  }
  if (body.model === 'fixture-rate-limit') {
    return json(
      429,
      { error: { type: 'rate_limit_error', message: 'Fixture rate limit.' } },
      { 'Retry-After': '2', 'x-request-id': 'fixture-request' },
    );
  }
  if (body.model !== 'fixture-model' || body.messages?.[0]?.content !== 'fixture prompt') {
    return json(400, { error: { message: 'Payload was not preserved.' } });
  }
  if (!body.stream) {
    return json(
      200,
      claude
        ? { id: 'fixture-message', type: 'message', content: [{ type: 'text', text: 'fixture' }] }
        : {
            id: 'fixture-completion',
            choices: [{ message: { role: 'assistant', content: 'fixture' } }],
          },
    );
  }
  response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  response.write(
    claude
      ? 'event: content_block_delta\ndata: {"type":"content_block_delta","delta":{"type":"text_delta","text":"fixture"}}\n\n'
      : 'data: {"choices":[{"delta":{"content":"fixture"}}]}\n\n',
  );
  const timer = setTimeout(() => {
    response.end(
      claude ? 'event: message_stop\ndata: {"type":"message_stop"}\n\n' : 'data: [DONE]\n\n',
    );
  }, 350);
  response.on('close', () => clearTimeout(timer));
});

server.listen(process.env.PORT || 4500);
