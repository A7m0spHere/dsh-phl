import { describe, expect, it } from 'vitest'
import { draftIssues } from './wizardStore'
import type { DshVersion, InstanceDraft } from '@/types'

const draft = (patch: Partial<InstanceDraft> = {}): InstanceDraft => ({
  name: 'Dev',
  note: '',
  kind: 'development',
  hue: 0,
  versionId: 'dsh-0.1.6-alpha.1',
  runtimeId: 'node-22',
  templateId: 'blank',
  autoPort: true,
  port: 3080,
  copyFromId: null,
  apiInheritance: 'default',
  ...patch,
})

const version = (patch: Partial<DshVersion> = {}): DshVersion =>
  ({ id: 'dsh-0.1.6-alpha.1', name: '0.1.6-alpha.1', state: { kind: 'available' }, ...patch }) as DshVersion

describe('draftIssues', () => {
  it('passes a fully chosen, installable draft', () => {
    expect(draftIssues(draft(), [], version())).toEqual({})
  })

  it('blocks a GitHub-only version instead of promising a download', () => {
    // The row is in the ordinary resting state; only `pendingPublish` says npm
    // has nothing to fetch. Creating from it produced an instance that could
    // not start (2026-09-10 review #17).
    const issues = draftIssues(draft(), [], version({ pendingPublish: true }))
    expect(issues.version).toContain('仅在 GitHub 发布')
    expect(issues.version).toContain('0.1.6-alpha.1')
  })

  it('still reports an unchosen version when the catalog row is missing', () => {
    expect(draftIssues(draft({ versionId: null }), [], undefined).version).toBe('请选择一个 DSH 版本')
    expect(draftIssues(draft({ versionId: null }), [], version()).version).toBe('请选择一个 DSH 版本')
  })

  it('keeps the name / runtime / port rules in one place', () => {
    const issues = draftIssues(
      draft({ name: '  ', runtimeId: null, autoPort: false, port: 80 }),
      ['Taken'],
      version(),
    )
    expect(issues.name).toBe('请填写实例名称')
    expect(issues.runtime).toBe('请选择一个 Node Runtime')
    expect(issues.port).toBe('端口需要在 1024 – 65535 之间')
    expect(draftIssues(draft({ name: 'Taken' }), ['Taken'], version()).name).toBe('已存在同名实例')
  })
})
