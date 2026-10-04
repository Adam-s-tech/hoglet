// PASS/FAIL recorder. Every assertion is a named step; a failing step records
// observed vs expected and the suite keeps going, so one run shows every
// broken promise rather than the first.

import { mkdirSync, writeFileSync } from "node:fs";
import { isDeepStrictEqual, inspect } from "node:util";

export class AssertionFailure extends Error {
  constructor(message, observed, expected) {
    super(message);
    this.observed = observed;
    this.expected = expected;
  }
}

const show = (v) =>
  typeof v === "string" ? v : inspect(v, { depth: 6, breakLength: Infinity, maxStringLength: 400 });

export function fail(message, observed, expected) {
  throw new AssertionFailure(message, observed, expected);
}

export function eq(observed, expected, what = "value") {
  if (!isDeepStrictEqual(observed, expected)) fail(`${what} differs`, observed, expected);
  return observed;
}

export function ok(condition, what, observed, expected) {
  if (!condition) fail(what, observed, expected);
}

/// `observed` must contain every key of `expected` with a deep-equal value.
export function subset(observed, expected, what = "object") {
  if (observed === null || typeof observed !== "object") fail(`${what} is not an object`, observed, expected);
  const wrong = {};
  for (const [k, v] of Object.entries(expected)) {
    if (!isDeepStrictEqual(observed[k], v)) wrong[k] = observed[k];
  }
  if (Object.keys(wrong).length) fail(`${what} has wrong keys`, wrong, Object.fromEntries(Object.keys(wrong).map((k) => [k, expected[k]])));
  return observed;
}

export class Report {
  constructor(suite) {
    this.suite = suite;
    this.results = [];
    this.meta = {};
    this.started = Date.now();
  }

  header(line) {
    console.log(`\n== ${line} ==`);
  }

  info(line) {
    console.log(`      ${line}`);
  }

  /// Runs one assertion. Returns the step's value, or undefined on failure.
  async step(name, fn) {
    const t0 = Date.now();
    try {
      const value = await fn();
      this.results.push({ name, status: "PASS", ms: Date.now() - t0 });
      console.log(`PASS  ${name}`);
      return value;
    } catch (error) {
      const entry = { name, status: "FAIL", ms: Date.now() - t0, message: error?.message ?? String(error) };
      let line = `FAIL  ${name} — ${entry.message}`;
      if (error instanceof AssertionFailure) {
        entry.observed = error.observed;
        entry.expected = error.expected;
        line += `\n        observed: ${show(error.observed)}\n        expected: ${show(error.expected)}`;
      } else if (error?.stack && process.env.CONTRACT_VERBOSE) {
        line += `\n${error.stack}`;
      }
      this.results.push(entry);
      console.log(line);
      return undefined;
    }
  }

  /// Records a step that could not run because a prerequisite failed.
  skip(name, reason) {
    this.results.push({ name, status: "FAIL", message: `not run: ${reason}` });
    console.log(`FAIL  ${name} — not run: ${reason}`);
  }

  finish() {
    const passed = this.results.filter((r) => r.status === "PASS").length;
    const failed = this.results.length - passed;
    const secs = ((Date.now() - this.started) / 1000).toFixed(1);
    console.log(`\n${this.suite}: ${passed} passed, ${failed} failed (${secs}s)`);
    const dir = new URL("../results/", import.meta.url).pathname;
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      `${dir}${this.suite}.json`,
      JSON.stringify({ suite: this.suite, meta: this.meta, passed, failed, results: this.results }, null, 2)
    );
    return failed === 0 ? 0 : 1;
  }
}
