import { expect, it } from 'vitest'
import { buildAgentInstallTask, releaseUrl } from './githubBuildTask'

it('maps a version name to its deterministic tag URL', () => {
  expect(releaseUrl('0.1.3-alpha.1')).toBe(
    'https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.3-alpha.1',
  )
})

it('embeds tag, target dir, the pnpm-deploy path and the workspace hard gate', () => {
  const task = buildAgentInstallTask({
    versionName: '0.1.3-alpha.1',
    root: 'C:\\Users\\me\\AppData\\Local\\PHL\\',
    registryBase: 'https://registry.npmmirror.com',
  })
  expect(task).toContain('--branch dsh-v0.1.3-alpha.1')
  // trailing separators trimmed, no double slashes
  expect(task).toContain('AppData\\Local\\PHL/versions/0.1.3-alpha.1')
  expect(task).not.toContain('PHL//versions')
  expect(task).toContain('pnpm deploy --filter @deepseek-ai/dsh')
  expect(task).toContain('workspace:^')
  expect(task).toContain('--registry https://registry.npmmirror.com')
  expect(task).toContain('phl-install.json')
  // guardrails the precedents demand: approval before destructive ops, a
  // verification gate before declaring success, and no silent repo edits.
  expect(task).toContain('ask approval')
  expect(task).toContain('HARD GATE')
  expect(task).toContain('instead of editing the repo')
})

it('omits the registry flag when the user is on the default registry', () => {
  const task = buildAgentInstallTask({ versionName: '1.0.0', root: '/tmp/phl' })
  expect(task).not.toContain('--registry')
  expect(task).toContain('/tmp/phl/versions/1.0.0')
})
