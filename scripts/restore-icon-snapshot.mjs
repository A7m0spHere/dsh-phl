/**
 * Restores an icon snapshot over src-tauri/icons.
 *
 *   node scripts/restore-icon-snapshot.mjs crisp-20px-approved
 *
 * Snapshots live in src-tauri/icons/snapshots/<name>/ and are the
 * byte-exact rollback path when a small-size experiment turns out worse.
 */
import { copyFileSync, existsSync, readdirSync, statSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const SNAP_DIR = resolve(ROOT, 'src-tauri/icons/snapshots')
const OUT_DIR = resolve(ROOT, 'src-tauri/icons')

const name = process.argv[2]
if (!name) {
  const available = existsSync(SNAP_DIR) ? readdirSync(SNAP_DIR).join(', ') : '(none)'
  console.error('usage: node scripts/restore-icon-snapshot.mjs <name>')
  console.error('available: ' + available)
  process.exit(1)
}
const src = resolve(SNAP_DIR, name)
if (!existsSync(src)) {
  console.error('snapshot not found: ' + src)
  process.exit(1)
}

let restored = 0
for (const entry of readdirSync(src, { withFileTypes: true })) {
  if (!entry.isFile()) continue
  if (entry.name === 'SNAPSHOT.md') continue
  const file = resolve(src, entry.name)
  if (!statSync(file).isFile()) continue
  copyFileSync(file, resolve(OUT_DIR, entry.name))
  restored++
}
console.log('restored ' + restored + ' files from snapshot ' + name + ' -> ' + OUT_DIR)
console.log('run `npm run app:dev` (or rebuild) to see them in the window/taskbar.')
