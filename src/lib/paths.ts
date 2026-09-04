/** Trim trailing separators without turning filesystem roots into relative paths. */
export const normalizeRoot = (raw: string) => {
  const trimmed = raw.trim()
  if (/^[a-z]:[\\/]+$/i.test(trimmed)) return `${trimmed.slice(0, 2)}/`
  if (/^[\\/]+$/.test(trimmed)) return trimmed[0]
  return trimmed.replace(/[\\/]+$/, '')
}
