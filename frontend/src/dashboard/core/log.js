// Scoped logger. See planning/dashboard/08-conventions.md § Logging.

const levels = { debug: 10, info: 20, warn: 30, error: 40 };
let threshold = levels.info;

export function setLogLevel(level) {
  const n = levels[level];
  if (n != null) threshold = n;
}

function emit(level, scope, args) {
  if (levels[level] < threshold) return;
  const fn = console[level === 'debug' ? 'log' : level];
  fn(`[${scope}]`, ...args);
}

export function log(scope) {
  return {
    debug: (...args) => emit('debug', scope, args),
    info:  (...args) => emit('info',  scope, args),
    warn:  (...args) => emit('warn',  scope, args),
    error: (...args) => emit('error', scope, args),
  };
}
