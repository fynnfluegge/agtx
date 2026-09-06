import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// Exercise the actual socket controller with browser transport/timer fakes.
const source = readFileSync(new URL('../web/app.js', import.meta.url), 'utf8');
function setup() {
  const connections = [];
  const timers = new Map();
  let nextTimer = 0;
  class WebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    constructor() {
      this.readyState = WebSocket.CONNECTING;
      this.handlers = {};
      this.sent = [];
      connections.push(this);
    }
    addEventListener(event, handler) { this.handlers[event] = handler; }
    send(message) { this.sent.push(JSON.parse(message)); }
    close() { this.readyState = 3; }
    emit(event) { this.handlers[event]?.({}); }
  }
  const context = vm.createContext({
    WebSocket, location: { protocol: 'https:', host: 'board.test' },
    wsProtocols: () => [], paintPane() {},
    setTimeout(fn) { const id = ++nextTimer; timers.set(id, fn); return id; },
    clearTimeout(id) { timers.delete(id); },
  });
  const socketState = source.slice(source.indexOf('let socket = null;'), source.indexOf('/// A live terminal.'));
  const controller = source.slice(source.indexOf('function openSocket('), source.indexOf('// ── boot'));
  vm.runInContext(socketState + controller, context);
  const run = (code) => vm.runInContext(code, context);
  run("openSocket('project', 'task', {}, {})");
  return { connections, timers, run, context };
}

test('disconnect retries and subscribes to the same task', () => {
  const { connections, timers } = setup();
  connections[0].readyState = 3;
  connections[0].emit('close');
  assert.equal(timers.size, 1);
  [...timers.values()][0]();
  assert.equal(connections.length, 2);
  connections[1].readyState = 1;
  connections[1].emit('open');
  assert.deepEqual(connections[1].sent, [{ type: 'subscribe', project_id: 'project', task_id: 'task' }]);
});

test('leaving the terminal cancels pending reconnection', () => {
  const { connections, timers, run } = setup();
  connections[0].readyState = 3;
  connections[0].emit('close');
  const queued = [...timers.values()][0];
  run('closeSocket()');
  assert.equal(timers.size, 0);
  queued(); // Even a timer already queued by the browser cannot reopen it.
  assert.equal(connections.length, 1);
});

test('refresh replaces a suspended socket without replacing input DOM', async () => {
  const { connections, run, context } = setup();
  connections[0].readyState = 1;
  const screen = source.slice(source.indexOf('async function screenTask('), source.indexOf('function detailsBody('));
  run(`let taskTab = 'terminal'; let mountedTask = 'project/task/terminal'; ${screen}`);
  run('globalThis.originalPane = term.pane');
  await vm.runInContext("screenTask('project', 'task')", context);
  assert.equal(connections.length, 2);
  assert.equal(run('term.pane === originalPane'), true);
  assert.equal(connections[0].readyState, 3);
});
