#!/usr/bin/env node

/**
 * Names/metadata-only fake Codex executable for the real-spawn boundary test.
 * It never reads environment values and never opens a network connection.
 */

process.stdin.resume();
process.stdin.once('end', () => {
  const metadata = {
    argv: process.argv.slice(2),
    env_names: Object.keys(process.env).sort(),
  };

  const events = [
    { type: 'thread.started', thread_id: 'thread-real-spawn' },
    { type: 'turn.started' },
    {
      type: 'item.completed',
      item: {
        id: 'metadata',
        type: 'agent_message',
        text: JSON.stringify(metadata),
      },
    },
    {
      type: 'turn.completed',
      usage: {
        input_tokens: 0,
        cached_input_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
      },
    },
  ];

  for (const event of events) process.stdout.write(`${JSON.stringify(event)}\n`);
});
