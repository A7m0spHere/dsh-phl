import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'

const root = process.cwd()
const libDir = path.join(root, 'src', 'lib')
const bridgeFiles = fs
  .readdirSync(libDir)
  .filter((name) => /^desktop.*\.ts$/.test(name))
  .map((name) => path.join(libDir, name))

const invocations = new Map()
const invokePattern = /\binvoke(?:<[^;]*?>)?\s*\(\s*['"]([^'"]+)['"]/g
for (const file of bridgeFiles) {
  const source = fs.readFileSync(file, 'utf8')
  for (const match of source.matchAll(invokePattern)) {
    const command = match[1]
    if (command.startsWith('plugin:')) continue
    const relative = path.relative(root, file).replaceAll('\\', '/')
    const row = invocations.get(command) ?? []
    row.push(relative)
    invocations.set(command, row)
  }
}

const rust = fs.readFileSync(path.join(root, 'src-tauri', 'src', 'lib.rs'), 'utf8')
// Registrations live in two shapes: the shared `phl_command_handler!` macro
// body (the runtime-agnostic list both `run()` and `build_app` use) and the
// `phl_command_handler![...]` invocation in `run()` that appends the Wry-bound
// commands. Both are real registrations, so both are collected.
const handlerBlocks = [
  ...rust.matchAll(/generate_handler!\s*\[([\s\S]*?)\]/g),
  ...rust.matchAll(/phl_command_handler!\s*\[([\s\S]*?)\]/g),
]
if (handlerBlocks.length === 0) throw new Error('No Tauri generate_handler! block found')

const registered = new Map()
for (const block of handlerBlocks) {
  // Strip comments (the lists are documented inline) and the macro's
  // `$($extra),*` placeholder — neither is a command name.
  const body = block[1]
    .replace(/\/\/[^\n]*/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\$\([^)]*\)[^,]*/g, '')
  for (const match of body.matchAll(/(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)/g)) {
    const command = match[1]
    registered.set(command, (registered.get(command) ?? 0) + 1)
  }
}

const missing = [...invocations.keys()].filter((command) => !registered.has(command)).sort()
const duplicates = [...registered.entries()].filter(([, count]) => count > 1).map(([command]) => command).sort()
const report = {
  bridgeFiles: bridgeFiles.length,
  invokedCommands: invocations.size,
  registeredCommands: registered.size,
  missing,
  duplicateRegistrations: duplicates,
}

console.log(JSON.stringify(report, null, 2))
if (missing.length || duplicates.length) process.exitCode = 1
