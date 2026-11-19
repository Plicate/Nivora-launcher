export function chooseAllowedCatalog(allowed: string[], preferred: string, current: string): string | undefined {
  if (allowed.includes(preferred)) return preferred
  if (allowed.includes(current)) return current
  return allowed[0]
}
