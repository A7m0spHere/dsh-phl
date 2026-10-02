import * as desktop from '@/lib/desktop'
import type { PhlRepository } from './repository'

/**
 * Desktop overrides for the **official desktop singleton** module
 * (`src-tauri/src/official/`). All three are backend-side probes/actions on
 * the user's real official DeepSeek Harness install; the browser mock never
 * fakes them (see `mockRepository`).
 */
export const tauriOfficialDesktopOverrides: Pick<
  PhlRepository,
  'officialDesktop' | 'launchOfficialDesktop' | 'quitOfficialDesktop'
> = {
  officialDesktop: () => desktop.inspectOfficialDesktop(),
  launchOfficialDesktop: () => desktop.launchOfficialDesktop(),
  quitOfficialDesktop: (force) => desktop.quitOfficialDesktop(force),
}
