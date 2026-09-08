#!/usr/bin/env node
/**
 * Mark a desktop scenario result into the alpha-desktop report:
 *
 *   node scripts/alpha-desktop-record.mjs <report.json> <scenario> <step#> <pass|fail> [note…]
 *
 * The report is written by alpha-desktop.mjs next to the scratch root. Keeping
 * marks in a file means a Computer-Use (or human) pass produces evidence: a
 * re-runnable record, not a vibe.
 */
import fs from 'node:fs'
import process from 'node:process'

const [reportPath, scenario, stepNum, verdict, ...noteParts] = process.argv.slice(2)
if (!reportPath || !scenario || !stepNum || !['pass', 'fail', 'skip', 'blocked'].includes(verdict)) {
  process.stderr.write('usage: alpha-desktop-record.mjs <report.json> <scenario> <step#> <pass|fail|skip> [note…]\n')
  process.exit(2)
}
const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'))
report.results ??= []
report.results.push({
  scenario,
  step: Number(stepNum),
  verdict,
  note: noteParts.join(' ') || undefined,
  at: new Date().toISOString(),
})
fs.writeFileSync(reportPath, JSON.stringify(report, null, 2))
const fails = report.results.filter((r) => r.verdict === 'fail').length
process.stdout.write(`recorded ${scenario} step ${stepNum} → ${verdict} (total: ${report.results.length} marked, ${fails} failing)\n`)
