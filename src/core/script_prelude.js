// Postman-compatible `pm.*` runtime for request scripts (see src/core/script.rs).
//
// Inputs (set by Rust before this file runs):
//   globalThis.__input_json    JSON: { stage, request, env, collection, response }
//   globalThis.__response_body response body text, or null
// Output: globalThis.__finish() returns the JSON the Rust side deserializes.
(function () {
  'use strict';

  var G = globalThis;
  var jsonStringify = JSON.stringify;
  var jsonParse = JSON.parse;
  var input = jsonParse(G.__input_json);
  var bodyText = typeof G.__response_body === 'string' ? G.__response_body : '';
  var hasOwn = function (o, k) { return Object.prototype.hasOwnProperty.call(o, k); };

  // ---- output ---------------------------------------------------------------

  function ChangeLog() { this.list = []; this.index = new Map(); }
  ChangeLog.prototype.record = function (key, value) {
    if (this.index.has(key)) {
      this.list[this.index.get(key)].value = value;
    } else {
      this.index.set(key, this.list.length);
      this.list.push({ key: key, value: value });
    }
  };

  var envChanges = new ChangeLog();
  var collChanges = new ChangeLog();
  var tests = [];
  var logs = [];

  // ---- helpers --------------------------------------------------------------

  // Postman stores variables as strings in our model: coerce like it does.
  function toVarString(v) {
    if (v === undefined || v === null) return '';
    if (typeof v === 'string') return v;
    if (typeof v === 'object') {
      try { return jsonStringify(v); } catch (e) { return String(v); }
    }
    return String(v);
  }

  function formatArg(v) {
    if (typeof v === 'string') return v;
    if (v === undefined) return 'undefined';
    if (v instanceof Error) return v.name + ': ' + v.message;
    if (typeof v === 'function') return '[Function' + (v.name ? ': ' + v.name : '') + ']';
    if (typeof v === 'object' && v !== null) {
      try {
        var s = jsonStringify(v);
        return s === undefined ? String(v) : s;
      } catch (e) {
        return String(v);
      }
    }
    return String(v);
  }

  function inspect(v) {
    if (typeof v === 'string') return "'" + v + "'";
    if (v === undefined) return 'undefined';
    if (typeof v === 'number' && isNaN(v)) return 'NaN';
    if (v instanceof RegExp) return String(v);
    if (typeof v === 'function') return '[Function' + (v.name ? ': ' + v.name : '') + ']';
    if (typeof v === 'object' && v !== null) {
      try { return jsonStringify(v); } catch (e) { return String(v); }
    }
    return String(v);
  }

  function typeName(v) {
    if (v === null) return 'null';
    if (Array.isArray(v)) return 'array';
    if (v instanceof RegExp) return 'regexp';
    if (v instanceof Date) return 'date';
    return typeof v;
  }

  function deepEqual(a, b) {
    if (a === b) return true;
    if (typeof a === 'number' && typeof b === 'number') return isNaN(a) && isNaN(b);
    if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    if (a instanceof Date && b instanceof Date) return a.getTime() === b.getTime();
    if (a instanceof RegExp && b instanceof RegExp) return String(a) === String(b);
    var ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (var i = 0; i < ka.length; i++) {
      if (!hasOwn(b, ka[i]) || !deepEqual(a[ka[i]], b[ka[i]])) return false;
    }
    return true;
  }

  function replaceVars(template, lookup) {
    if (typeof template !== 'string') return template;
    return template.replace(/\{\{([^{}]+)\}\}/g, function (whole, name) {
      var v = lookup(name.trim());
      return v === undefined ? whole : v;
    });
  }

  // ---- variables ------------------------------------------------------------

  function toMap(obj) {
    var m = new Map();
    if (obj) Object.keys(obj).forEach(function (k) { m.set(k, obj[k]); });
    return m;
  }

  var envStore = toMap(input.env);
  var collStore = toMap(input.collection);
  var localStore = new Map();

  function mapToObject(m) {
    var o = {};
    m.forEach(function (v, k) { o[k] = v; });
    return o;
  }

  function persistentScope(store, changes) {
    var scope = {
      get: function (key) { return store.has(key) ? store.get(key) : undefined; },
      set: function (key, value) {
        key = String(key);
        var v = toVarString(value);
        store.set(key, v);
        changes.record(key, v);
      },
      unset: function (key) {
        key = String(key);
        store.delete(key);
        changes.record(key, null);
      },
      has: function (key) { return store.has(key); },
      clear: function () {
        Array.from(store.keys()).forEach(function (k) { scope.unset(k); });
      },
      toObject: function () { return mapToObject(store); },
      replaceIn: function (template) { return replaceVars(template, scope.get); }
    };
    return scope;
  }

  var environment = persistentScope(envStore, envChanges);
  var collectionVariables = persistentScope(collStore, collChanges);

  function lookupVariable(key) {
    if (localStore.has(key)) return localStore.get(key);
    if (collStore.has(key)) return collStore.get(key);
    if (envStore.has(key)) return envStore.get(key);
    return undefined;
  }

  var variables = {
    get: lookupVariable,
    set: function (key, value) { localStore.set(String(key), toVarString(value)); },
    unset: function (key) { localStore.delete(String(key)); },
    has: function (key) { return lookupVariable(key) !== undefined; },
    toObject: function () {
      var o = mapToObject(envStore);
      collStore.forEach(function (v, k) { o[k] = v; });
      localStore.forEach(function (v, k) { o[k] = v; });
      return o;
    },
    replaceIn: function (template) { return replaceVars(template, lookupVariable); }
  };

  // ---- console --------------------------------------------------------------

  function logger(level) {
    return function () {
      var parts = [];
      for (var i = 0; i < arguments.length; i++) parts.push(formatArg(arguments[i]));
      logs.push({ level: level, text: parts.join(' ') });
    };
  }

  var console = {
    log: logger('log'),
    info: logger('info'),
    warn: logger('warn'),
    error: logger('error'),
    debug: logger('log'),
    trace: logger('log')
  };

  // ---- assertions (chai-lite) -----------------------------------------------

  class AssertionError extends Error {
    constructor(message) {
      super(message);
      this.name = 'AssertionError';
    }
  }

  function Assertion(obj, message) {
    this._obj = obj;
    this._negate = false;
    this._deep = false;
    this._message = message;
  }

  Assertion.prototype._assert = function (ok, positive, negative) {
    if (this._negate ? ok : !ok) {
      var text = this._negate ? negative : positive;
      throw new AssertionError(this._message ? this._message + ': ' + text : text);
    }
    return this;
  };

  function define(name, getter) {
    Object.defineProperty(Assertion.prototype, name, { get: getter, configurable: true });
  }

  ['to', 'be', 'been', 'is', 'that', 'which', 'and', 'has', 'have', 'with', 'at', 'of',
   'same', 'but', 'does', 'still', 'also', 'any', 'all', 'own', 'nested', 'ordered']
    .forEach(function (chain) { define(chain, function () { return this; }); });

  define('not', function () { this._negate = !this._negate; return this; });
  define('deep', function () { this._deep = true; return this; });

  define('ok', function () {
    return this._assert(!!this._obj, 'expected ' + inspect(this._obj) + ' to be truthy',
      'expected ' + inspect(this._obj) + ' to be falsy');
  });
  define('true', function () {
    return this._assert(this._obj === true, 'expected ' + inspect(this._obj) + ' to be true',
      'expected ' + inspect(this._obj) + ' to not be true');
  });
  define('false', function () {
    return this._assert(this._obj === false, 'expected ' + inspect(this._obj) + ' to be false',
      'expected ' + inspect(this._obj) + ' to not be false');
  });
  define('null', function () {
    return this._assert(this._obj === null, 'expected ' + inspect(this._obj) + ' to be null',
      'expected ' + inspect(this._obj) + ' to not be null');
  });
  define('undefined', function () {
    return this._assert(this._obj === undefined, 'expected ' + inspect(this._obj) + ' to be undefined',
      'expected ' + inspect(this._obj) + ' to not be undefined');
  });
  define('NaN', function () {
    return this._assert(typeof this._obj === 'number' && isNaN(this._obj),
      'expected ' + inspect(this._obj) + ' to be NaN', 'expected ' + inspect(this._obj) + ' not to be NaN');
  });
  define('exist', function () {
    return this._assert(this._obj !== null && this._obj !== undefined,
      'expected ' + inspect(this._obj) + ' to exist', 'expected ' + inspect(this._obj) + ' to not exist');
  });
  define('empty', function () {
    var o = this._obj, empty;
    if (typeof o === 'string' || Array.isArray(o)) empty = o.length === 0;
    else if (o instanceof Map || o instanceof Set) empty = o.size === 0;
    else if (o !== null && typeof o === 'object') empty = Object.keys(o).length === 0;
    else throw new AssertionError('.empty was passed non-string primitive ' + inspect(o));
    return this._assert(empty, 'expected ' + inspect(o) + ' to be empty',
      'expected ' + inspect(o) + ' not to be empty');
  });

  var A = Assertion.prototype;

  A.equal = function (v) {
    if (this._deep) return this.eql(v);
    return this._assert(this._obj === v, 'expected ' + inspect(this._obj) + ' to equal ' + inspect(v),
      'expected ' + inspect(this._obj) + ' to not equal ' + inspect(v));
  };
  A.equals = A.eq = A.equal;

  A.eql = function (v) {
    return this._assert(deepEqual(this._obj, v),
      'expected ' + inspect(this._obj) + ' to deeply equal ' + inspect(v),
      'expected ' + inspect(this._obj) + ' to not deeply equal ' + inspect(v));
  };
  A.eqls = A.eql;

  A.include = function (v) {
    var o = this._obj, found, self = this;
    var same = function (x) { return self._deep ? deepEqual(x, v) : x === v; };
    if (typeof o === 'string') {
      found = o.indexOf(String(v)) !== -1;
    } else if (Array.isArray(o)) {
      found = o.some(same);
    } else if (o instanceof Set) {
      found = Array.from(o).some(same);
    } else if (o instanceof Map) {
      found = Array.from(o.values()).some(same);
    } else if (o !== null && typeof o === 'object') {
      if (v !== null && typeof v === 'object') {
        found = Object.keys(v).every(function (k) {
          return hasOwn(o, k) && (self._deep ? deepEqual(o[k], v[k]) : o[k] === v[k]);
        });
      } else {
        found = hasOwn(o, String(v));
      }
    } else {
      throw new AssertionError('the given combination of arguments (' + typeName(o) + ' and ' +
        typeName(v) + ') is invalid for this assertion');
    }
    return this._assert(found, 'expected ' + inspect(o) + ' to include ' + inspect(v),
      'expected ' + inspect(o) + ' to not include ' + inspect(v));
  };
  A.includes = A.contain = A.contains = A.include;

  A.string = function (s) {
    return this._assert(typeof this._obj === 'string' && this._obj.indexOf(s) !== -1,
      'expected ' + inspect(this._obj) + ' to contain ' + inspect(s),
      'expected ' + inspect(this._obj) + ' to not contain ' + inspect(s));
  };

  A.property = function (name, value) {
    var o = this._obj;
    var has = o !== null && o !== undefined && (typeof o === 'object' || typeof o === 'function'
      ? name in Object(o) : Object(o)[name] !== undefined);
    if (arguments.length > 1) {
      var matches = has && (this._deep ? deepEqual(o[name], value) : o[name] === value);
      this._assert(matches,
        'expected ' + inspect(o) + ' to have property ' + inspect(name) + ' of ' + inspect(value) +
          (has ? ', but got ' + inspect(o[name]) : ''),
        'expected ' + inspect(o) + ' to not have property ' + inspect(name) + ' of ' + inspect(value));
    } else {
      this._assert(has, 'expected ' + inspect(o) + ' to have property ' + inspect(name),
        'expected ' + inspect(o) + ' to not have property ' + inspect(name));
    }
    if (!this._negate && has) this._obj = o[name];
    return this;
  };
  A.ownProperty = A.haveOwnProperty = A.property;

  A.keys = function () {
    var wanted = arguments.length === 1 && Array.isArray(arguments[0])
      ? arguments[0] : Array.prototype.slice.call(arguments);
    var o = this._obj;
    var actual = o !== null && typeof o === 'object' ? Object.keys(o) : [];
    var ok = wanted.every(function (k) { return actual.indexOf(String(k)) !== -1; });
    return this._assert(ok, 'expected ' + inspect(o) + ' to have keys ' + inspect(wanted),
      'expected ' + inspect(o) + ' to not have keys ' + inspect(wanted));
  };
  A.key = A.keys;

  A.lengthOf = function (n) {
    var o = this._obj;
    var len = o === null || o === undefined ? undefined : (o instanceof Map || o instanceof Set ? o.size : o.length);
    return this._assert(len === n, 'expected ' + inspect(o) + ' to have a length of ' + n + ' but got ' + len,
      'expected ' + inspect(o) + ' to not have a length of ' + n);
  };
  A.length = A.lengthOf;

  function compare(name, test, word) {
    A[name] = function (n) {
      return this._assert(test(this._obj, n), 'expected ' + inspect(this._obj) + ' to be ' + word + ' ' + inspect(n),
        'expected ' + inspect(this._obj) + ' to not be ' + word + ' ' + inspect(n));
    };
  }
  compare('above', function (a, b) { return a > b; }, 'above');
  compare('below', function (a, b) { return a < b; }, 'below');
  compare('least', function (a, b) { return a >= b; }, 'at least');
  compare('most', function (a, b) { return a <= b; }, 'at most');
  A.gt = A.greaterThan = A.above;
  A.lt = A.lessThan = A.below;
  A.gte = A.greaterThanOrEqual = A.least;
  A.lte = A.lessThanOrEqual = A.most;

  A.within = function (lo, hi) {
    var o = this._obj;
    return this._assert(o >= lo && o <= hi, 'expected ' + inspect(o) + ' to be within ' + lo + '..' + hi,
      'expected ' + inspect(o) + ' to not be within ' + lo + '..' + hi);
  };

  A.a = function (type) {
    var t = String(type).toLowerCase();
    var actual = typeName(this._obj);
    var article = /^[aeiou]/.test(t) ? 'an ' : 'a ';
    return this._assert(actual === t, 'expected ' + inspect(this._obj) + ' to be ' + article + t,
      'expected ' + inspect(this._obj) + ' not to be ' + article + t);
  };
  A.an = A.a;

  A.instanceOf = function (ctor) {
    return this._assert(this._obj instanceof ctor,
      'expected ' + inspect(this._obj) + ' to be an instance of ' + (ctor && ctor.name),
      'expected ' + inspect(this._obj) + ' to not be an instance of ' + (ctor && ctor.name));
  };
  A.instanceof = A.instanceOf;

  A.match = function (re) {
    return this._assert(re.test(String(this._obj)), 'expected ' + inspect(this._obj) + ' to match ' + re,
      'expected ' + inspect(this._obj) + ' not to match ' + re);
  };
  A.matches = A.match;

  A.oneOf = function (list) {
    var o = this._obj, self = this;
    var ok = list.some(function (x) { return self._deep ? deepEqual(x, o) : x === o; });
    return this._assert(ok, 'expected ' + inspect(o) + ' to be one of ' + inspect(list),
      'expected ' + inspect(o) + ' to not be one of ' + inspect(list));
  };

  A.members = function (list) {
    var o = this._obj, self = this;
    var ok = Array.isArray(o) && o.length === list.length && list.every(function (x) {
      return o.some(function (y) { return self._deep ? deepEqual(x, y) : x === y; });
    });
    return this._assert(ok, 'expected ' + inspect(o) + ' to have the same members as ' + inspect(list),
      'expected ' + inspect(o) + ' to not have the same members as ' + inspect(list));
  };

  // chai-postman: pm.expect(pm.response).to.have.status(200)
  A.status = function (code) {
    var o = this._obj || {};
    var ok = typeof code === 'number' ? o.code === code : o.status === code;
    return this._assert(ok, 'expected response to have status code ' + inspect(code) + ' but got ' + inspect(o.code),
      'expected response to not have status code ' + inspect(code));
  };

  A['throw'] = function () {
    var threw = false;
    try { this._obj(); } catch (e) { threw = true; }
    return this._assert(threw, 'expected ' + inspect(this._obj) + ' to throw an error',
      'expected ' + inspect(this._obj) + ' to not throw an error');
  };
  A.throws = A.Throw = A['throw'];

  A.fail = function (message) { throw new AssertionError(message || 'assertion failed'); };

  function expect(value, message) { return new Assertion(value, message); }
  expect.fail = function (message) { throw new AssertionError(message || 'expect.fail()'); };

  // ---- tests ----------------------------------------------------------------

  function errorText(e) {
    if (e instanceof AssertionError) return 'AssertionError: ' + e.message;
    if (e instanceof Error) return (e.name || 'Error') + ': ' + e.message;
    return formatArg(e);
  }

  function test(name, fn) {
    name = String(name);
    if (typeof fn !== 'function') {
      tests.push({ name: name, passed: true, error: null });
      return;
    }
    try {
      fn();
      tests.push({ name: name, passed: true, error: null });
    } catch (e) {
      tests.push({ name: name, passed: false, error: errorText(e) });
    }
  }
  test.skip = function () {};

  // ---- request --------------------------------------------------------------

  var req = input.request;
  var reqState = {
    url: req.url,
    method: req.method,
    headers: req.headers.map(function (h, i) {
      return { key: h.key, value: h.value, enabled: h.enabled, index: i };
    }),
    bodyRaw: req.body.raw,
    bodyChanged: false
  };

  function headerMatches(h, name) {
    return String(h.key).toLowerCase() === String(name).toLowerCase();
  }

  function parseHeaderArg(h, value) {
    if (typeof h === 'string') {
      if (value !== undefined) return { key: h, value: toVarString(value) };
      var colon = h.indexOf(':');
      return colon === -1 ? { key: h.trim(), value: '' }
        : { key: h.slice(0, colon).trim(), value: h.slice(colon + 1).trim() };
    }
    return { key: String(h.key), value: toVarString(h.value), disabled: !!h.disabled };
  }

  var requestHeaders = {
    add: function (h, value) {
      var p = parseHeaderArg(h, value);
      reqState.headers.push({ key: p.key, value: p.value, enabled: !p.disabled, index: null });
    },
    upsert: function (h, value) {
      var p = parseHeaderArg(h, value);
      for (var i = 0; i < reqState.headers.length; i++) {
        if (headerMatches(reqState.headers[i], p.key)) {
          reqState.headers[i].value = p.value;
          reqState.headers[i].enabled = !p.disabled;
          return;
        }
      }
      reqState.headers.push({ key: p.key, value: p.value, enabled: !p.disabled, index: null });
    },
    remove: function (key) {
      if (key && typeof key === 'object') key = key.key;
      reqState.headers = reqState.headers.filter(function (h) { return !headerMatches(h, key); });
    },
    get: function (key) {
      for (var i = 0; i < reqState.headers.length; i++) {
        var h = reqState.headers[i];
        if (h.enabled && headerMatches(h, key)) return h.value;
      }
      return undefined;
    },
    has: function (key) { return requestHeaders.get(key) !== undefined; },
    toObject: function () {
      var o = {};
      reqState.headers.forEach(function (h) { if (h.enabled) o[h.key] = h.value; });
      return o;
    },
    all: function () {
      return reqState.headers.filter(function (h) { return h.enabled; })
        .map(function (h) { return { key: h.key, value: h.value }; });
    },
    each: function (fn) { requestHeaders.all().forEach(fn); },
    count: function () { return requestHeaders.all().length; }
  };

  var requestUrl = {
    toString: function () { return reqState.url; },
    getRaw: function () { return reqState.url; },
    update: function (v) { reqState.url = String(v); },
    valueOf: function () { return reqState.url; },
    toJSON: function () { return reqState.url; }
  };
  Object.defineProperty(requestUrl, 'raw', {
    get: function () { return reqState.url; },
    set: function (v) { reqState.url = String(v); }
  });

  var requestBody = {
    get mode() { return req.body.mode; },
    toString: function () { return reqState.bodyRaw; },
    update: function (v) {
      reqState.bodyRaw = typeof v === 'string' ? v : (v && typeof v.raw === 'string' ? v.raw : toVarString(v));
      reqState.bodyChanged = true;
    }
  };
  Object.defineProperty(requestBody, 'raw', {
    get: function () { return reqState.bodyRaw; },
    set: function (v) { reqState.bodyRaw = toVarString(v); reqState.bodyChanged = true; },
    enumerable: true
  });

  var request = {
    get name() { return req.name; },
    get id() { return req.id; },
    headers: requestHeaders,
    body: requestBody,
    addHeader: requestHeaders.add,
    removeHeader: requestHeaders.remove,
    upsertHeader: requestHeaders.upsert
  };
  Object.defineProperty(request, 'url', {
    get: function () { return requestUrl; },
    set: function (v) { reqState.url = String(v); },
    enumerable: true
  });
  Object.defineProperty(request, 'method', {
    get: function () { return reqState.method; },
    set: function (v) { reqState.method = String(v).toUpperCase(); },
    enumerable: true
  });

  // ---- response -------------------------------------------------------------

  var res = input.response;
  var response = null;
  if (res) {
    var findHeader = function (name) {
      for (var i = 0; i < res.headers.length; i++) {
        if (String(res.headers[i][0]).toLowerCase() === String(name).toLowerCase()) return res.headers[i][1];
      }
      return undefined;
    };
    var parsedJson;
    var parsedOnce = false;
    var responseHeaders = {
      get: findHeader,
      has: function (name, value) {
        var v = findHeader(name);
        return v !== undefined && (value === undefined || v === value);
      },
      toObject: function () {
        var o = {};
        res.headers.forEach(function (h) { o[h[0].toLowerCase()] = h[1]; });
        return o;
      },
      all: function () { return res.headers.map(function (h) { return { key: h[0], value: h[1] }; }); },
      each: function (fn) { responseHeaders.all().forEach(fn); },
      count: function () { return res.headers.length; }
    };
    var fail = function (message) { throw new AssertionError(message); };
    response = {
      code: res.code,
      status: res.status,
      responseTime: res.responseTime,
      responseSize: res.size,
      headers: responseHeaders,
      text: function () { return bodyText; },
      json: function () {
        if (!parsedOnce) {
          parsedJson = jsonParse(bodyText);
          parsedOnce = true;
        }
        return parsedJson;
      },
      reason: function () { return res.status; },
      to: {
        have: {
          status: function (code) {
            if (typeof code === 'number') {
              if (res.code !== code) fail('expected response to have status code ' + code + ' but got ' + res.code);
            } else if (res.status !== code) {
              fail('expected response to have status reason ' + inspect(code) + ' but got ' + inspect(res.status));
            }
          },
          header: function (name, value) {
            var v = findHeader(name);
            if (v === undefined) fail('expected response to have header with key ' + inspect(name));
            if (value !== undefined && v !== value) {
              fail('expected response header ' + inspect(name) + ' to have value ' + inspect(value) + ' but got ' + inspect(v));
            }
          },
          body: function (expected) {
            if (expected === undefined) {
              if (bodyText.length === 0) fail('expected response to have content in body');
            } else if (expected instanceof RegExp ? !expected.test(bodyText) : bodyText !== expected) {
              fail('expected response body to equal ' + inspect(expected));
            }
          },
          jsonBody: function (path, value) {
            var data;
            try { data = jsonParse(bodyText); } catch (e) { fail('expected response body to be a valid json'); }
            if (path === undefined) return;
            var cur = data;
            String(path).split('.').forEach(function (part) { cur = cur === null || cur === undefined ? undefined : cur[part]; });
            if (arguments.length > 1 ? !deepEqual(cur, value) : cur === undefined) {
              fail('expected response json body to have path ' + inspect(path) +
                (arguments.length > 1 ? ' with value ' + inspect(value) : ''));
            }
          }
        },
        be: {}
      }
    };
    var statusCheck = function (name, test, what) {
      Object.defineProperty(response.to.be, name, {
        get: function () {
          if (!test(res.code)) fail('expected response code to be ' + what + ' but found ' + res.code);
          return true;
        }
      });
    };
    statusCheck('ok', function (c) { return c === 200; }, '200');
    statusCheck('success', function (c) { return c >= 200 && c < 300; }, '2XX');
    statusCheck('info', function (c) { return c >= 100 && c < 200; }, '1XX');
    statusCheck('redirection', function (c) { return c >= 300 && c < 400; }, '3XX');
    statusCheck('clientError', function (c) { return c >= 400 && c < 500; }, '4XX');
    statusCheck('serverError', function (c) { return c >= 500; }, '5XX');
    statusCheck('error', function (c) { return c >= 400; }, '4XX or 5XX');
    statusCheck('notFound', function (c) { return c === 404; }, '404');
    statusCheck('unauthorized', function (c) { return c === 401; }, '401');
    statusCheck('forbidden', function (c) { return c === 403; }, '403');
    statusCheck('badRequest', function (c) { return c === 400; }, '400');
    statusCheck('accepted', function (c) { return c === 202; }, '202');
  }

  // ---- pm -------------------------------------------------------------------

  var pm = {
    environment: environment,
    collectionVariables: collectionVariables,
    globals: environment,
    variables: variables,
    request: request,
    response: response,
    info: {
      requestName: req.name,
      requestId: req.id,
      eventName: input.stage,
      iteration: 0,
      iterationCount: 1
    },
    test: test,
    expect: expect,
    sendRequest: function () { throw new Error('pm.sendRequest is not supported'); }
  };

  // ---- legacy API -----------------------------------------------------------

  var legacyTests = {};
  var postman = {
    setEnvironmentVariable: environment.set,
    getEnvironmentVariable: environment.get,
    clearEnvironmentVariable: environment.unset,
    clearEnvironmentVariables: environment.clear,
    setGlobalVariable: environment.set,
    getGlobalVariable: environment.get,
    clearGlobalVariable: environment.unset,
    getResponseHeader: function (name) { return response ? response.headers.get(name) : undefined; }
  };

  function b64chars() { return 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; }
  function btoa(s) {
    s = String(s);
    var chars = b64chars(), out = '';
    for (var i = 0; i < s.length; i += 3) {
      var a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new Error('btoa: string contains characters outside of the Latin1 range');
      var n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += chars[(n >> 18) & 63] + chars[(n >> 12) & 63] +
        (i + 1 < s.length ? chars[(n >> 6) & 63] : '=') + (i + 2 < s.length ? chars[n & 63] : '=');
    }
    return out;
  }
  function atob(s) {
    s = String(s).replace(/[\s=]/g, '');
    var chars = b64chars(), out = '', bits = 0, acc = 0;
    for (var i = 0; i < s.length; i++) {
      var v = chars.indexOf(s[i]);
      if (v === -1) throw new Error('atob: invalid character');
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 255);
      }
    }
    return out;
  }

  var globals = {
    pm: pm,
    postman: postman,
    console: console,
    tests: legacyTests,
    environment: environment.toObject(),
    globals: {},
    data: {},
    iteration: 0,
    responseBody: res ? bodyText : undefined,
    responseCode: res ? { code: res.code, name: res.status, detail: res.status } : undefined,
    responseTime: res ? res.responseTime : undefined,
    responseHeaders: res ? response.headers.toObject() : undefined,
    request: { url: req.url, method: req.method, name: req.name, headers: requestHeaders.toObject(), data: req.body.raw },
    btoa: btoa,
    atob: atob
  };
  Object.keys(globals).forEach(function (k) {
    Object.defineProperty(G, k, { value: globals[k], writable: true, configurable: true, enumerable: false });
  });

  // ---- finish ---------------------------------------------------------------

  function finish() {
    Object.keys(legacyTests).forEach(function (name) {
      var passed = !!legacyTests[name];
      tests.push({ name: name, passed: passed, error: passed ? null : 'expected true but got ' + inspect(legacyTests[name]) });
    });
    var request = null;
    if (input.stage === 'prerequest') {
      request = {
        url: String(reqState.url),
        method: String(reqState.method),
        headers: reqState.headers.map(function (h) {
          return { key: String(h.key), value: toVarString(h.value), enabled: !!h.enabled, index: h.index };
        }),
        body_raw: reqState.bodyChanged ? String(reqState.bodyRaw) : null
      };
    }
    return jsonStringify({
      env: envChanges.list,
      collection: collChanges.list,
      tests: tests,
      logs: logs,
      request: request
    });
  }

  Object.defineProperty(G, '__finish', { value: finish, writable: false, configurable: false });
  delete G.__input_json;
  delete G.__response_body;
})();
