/**
 * The "let the agent build it from GitHub" escape hatch for pending
 * releases (GitHub cut the tag; npm hasn't published the package yet).
 *
 * PHL does not run the build itself and does not auto-inject the task into a
 * DSH conversation — the mature precedents (Claude Code, Codex, OpenHands)
 * all keep a human approving what an agent executes. What PHL does instead:
 * generate a *verified-ready* install task for the tag (the facts below were
 * taken from the real repo at dsh-v0.1.3-alpha.1) and put it on the clipboard
 * next to a launched instance's WebUI. The user pastes, reviews, approves.
 */

const REPO = 'deepseek-ai/deepseek-harness'

/** Release page for a version name (`0.1.3-alpha.1` → tag `dsh-v0.1.3-alpha.1`). */
export function releaseUrl(versionName: string): string {
  return `https://github.com/${REPO}/releases/tag/dsh-v${versionName}`
}

export interface AgentTaskInput {
  versionName: string
  /** PHL data root (settingsStore.root), e.g. C:\Users\me\AppData\Local\PHL */
  root: string
  /** Registry mirror the user downloads from, so the build pulls deps the same way. */
  registryBase?: string
}

/** The prompt to paste into a running DSH agent. English: it drives a
 *  coding agent through a shell build; the host language is irrelevant. */
export function buildAgentInstallTask({ versionName, root, registryBase }: AgentTaskInput): string {
  const tag = `dsh-v${versionName}`
  // Trim trailing separators only; leave the platform's own separator alone —
  // the task is read by an agent that handles mixed slashes fine.
  const versionDir = `${root.replace(/[\\/]+$/, '')}/versions/${versionName}`
  const registryFlag = registryBase ? ` --registry ${registryBase}` : ''
  return [
    `Install DSH ${versionName} from source into PHL's shared versions directory, because the npm registry has not published this version yet.`,
    ``,
    `Source: https://github.com/${REPO} — tag \`${tag}\` (release: ${releaseUrl(versionName)}).`,
    `Target: ${versionDir} — PHL treats that directory as an installed version once it holds package.json, lib/bin.js, a populated node_modules and the marker described below. Do NOT touch any other directory under versions/.`,
    ``,
    `Steps (verify each before the next; ask approval before any destructive command):`,
    `1. Read the repo's CONTRIBUTING.md / AGENTS.md build section first; follow its authoritative instructions where they differ from this list.`,
    `2. git clone --branch ${tag} --depth 1 https://github.com/${REPO} <scratch>/dsh-build`,
    `3. cd in, then: corepack enable && pnpm install   (repo pins pnpm@11.7.0 via packageManager; approve its build scripts, e.g. esbuild)`,
    `4. pnpm build`,
    `5. Produce the install payload with ONE of:`,
    `   a) pnpm deploy --filter @deepseek-ai/dsh --prod ${versionDir}   (preferred: it resolves the internal workspace:^ dependencies into a real node_modules)`,
    `   b) fallback: pnpm --filter @deepseek-ai/dsh pack --pack-destination <scratch>, extract the package/ into ${versionDir}, then rewrite every "workspace:*"/"workspace:^" dependency in package.json to "^${versionName}" and run npm install --omit=dev --ignore-scripts --no-audit --no-package-lock${registryFlag}`,
    `6. HARD GATE before finishing: package.json under ${versionDir} must contain NO "workspace:" strings, node_modules must exist, and:`,
    `   node ${versionDir}/lib/bin.js --help   must run clean (module-resolution errors mean the payload is broken — fix before declaring success).`,
    `7. Write the marker file ${versionDir}/phl-install.json containing exactly: {"installedAt":"<current ISO8601 UTC>","source":"github-build"}`,
    ``,
    `If the build fails twice for the same reason, stop and summarize the error instead of editing the repo to make it pass — a modified build would silently diverge from the official release. Finish by reporting the final path and the step-6 verification output.`,
  ].join('\n')
}
