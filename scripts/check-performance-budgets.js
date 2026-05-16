#!/usr/bin/env node

const fs = require("fs");
const path = require("path");

function usage() {
  console.error(`Usage: scripts/check-performance-budgets.js --budget <file> --profile <name> (--result <file> | --result-dir <dir>)

Examples:
  scripts/check-performance-budgets.js --budget benchmarks/budgets/performance.json --profile audit-scale-100k --result /tmp/audit_scale.json
  scripts/check-performance-budgets.js --budget benchmarks/budgets/performance.json --profile audit-scale-100k --result-dir benchmarks/results
`);
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) {
      throw new Error(`unexpected argument: ${arg}`);
    }
    const key = arg.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    const value = argv[i + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`missing value for ${arg}`);
    }
    args[key] = value;
    i += 1;
  }
  return args;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function latestAuditScaleResult(dir) {
  const entries = fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && /^audit_scale_.*\.json$/.test(entry.name))
    .map((entry) => {
      const file = path.join(dir, entry.name);
      return { file, mtimeMs: fs.statSync(file).mtimeMs };
    })
    .sort((a, b) => b.mtimeMs - a.mtimeMs);

  if (entries.length === 0) {
    throw new Error(`no audit_scale_*.json files found in ${dir}`);
  }

  return entries[0].file;
}

function getPath(object, dottedPath) {
  return dottedPath.split(".").reduce((value, key) => {
    if (value === undefined || value === null) {
      return undefined;
    }
    return value[key];
  }, object);
}

function checkEqual(actual, expected, label, failures) {
  if (actual !== expected) {
    failures.push(`${label}: expected ${expected}, got ${actual}`);
  }
}

function checkGte(actual, expected, label, failures) {
  if (typeof actual !== "number" || actual < expected) {
    failures.push(`${label}: expected >= ${expected}, got ${actual}`);
  }
}

function main() {
  let args;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (error) {
    usage();
    throw error;
  }

  if (!args.budget || !args.profile || (!args.result && !args.resultDir)) {
    usage();
    process.exit(2);
  }

  const budgetFile = path.resolve(args.budget);
  const budget = readJson(budgetFile);
  const profile = budget.profiles && budget.profiles[args.profile];
  if (!profile) {
    throw new Error(`profile '${args.profile}' not found in ${budgetFile}`);
  }

  const resultFile = path.resolve(args.result || latestAuditScaleResult(args.resultDir));
  const result = readJson(resultFile);
  const failures = [];
  const warnings = [];

  const requirements = profile.requirements || {};
  if (requirements.events !== undefined) {
    checkEqual(result.events, requirements.events, "events", failures);
  }
  if (requirements.page_limit !== undefined) {
    checkEqual(result.page_limit, requirements.page_limit, "page_limit", failures);
  }
  if (requirements.violation_every !== undefined) {
    checkEqual(result.violation_every, requirements.violation_every, "violation_every", failures);
  }
  if (requirements.min_samples !== undefined) {
    checkGte(result.samples, requirements.min_samples, "samples", failures);
  }
  if (requirements.has_next_cursor !== undefined) {
    checkEqual(result.has_next_cursor, requirements.has_next_cursor, "has_next_cursor", failures);
  }

  for (const [metricPath, rule] of Object.entries(profile.metrics || {})) {
    const actual = getPath(result, metricPath);
    if (typeof actual !== "number") {
      failures.push(`${metricPath}: missing numeric value`);
      continue;
    }

    if (actual > rule.max) {
      const message = `${metricPath}: ${actual.toFixed(3)} ${rule.unit || ""} > budget ${rule.max} ${rule.unit || ""}`.trim();
      if (rule.severity === "warn") {
        warnings.push(message);
      } else {
        failures.push(message);
      }
    } else {
      console.log(`ok: ${metricPath} ${actual.toFixed(3)} <= ${rule.max} ${rule.unit || ""}`.trim());
    }
  }

  if (warnings.length > 0) {
    for (const warning of warnings) {
      console.warn(`WARN: ${warning}`);
    }
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`FAIL: ${failure}`);
    }
    console.error(`Performance budget failed for profile '${args.profile}' using ${resultFile}`);
    process.exit(1);
  }

  console.log(`Performance budget passed for profile '${args.profile}' using ${resultFile}`);
}

try {
  main();
} catch (error) {
  console.error(`ERROR: ${error.message}`);
  process.exit(1);
}
