import type { ModResult, ResourceCatalog } from '../types'

const demoResults: Record<'ftb' | 'technic', ModResult[]> = {
  ftb: [
    { project_id: 'preview-ftb-sky', title: 'FTB Preview — Sky', author: 'Feed The Beast', description: 'Демонстрационный результат предпросмотра; реальные данные доступны в desktop-версии.', downloads: 12400, project_type: 'modpack' },
    { project_id: 'preview-ftb-stone', title: 'FTB Preview — Stone', author: 'Feed The Beast', description: 'Демонстрационный результат предпросмотра; реальные данные доступны в desktop-версии.', downloads: 8600, project_type: 'modpack' },
  ],
  technic: [
    { project_id: 'preview-technic-classic', title: 'Technic Preview — Classic', author: 'Technic', description: 'Демонстрационный результат предпросмотра; реальные данные доступны в desktop-версии.', downloads: 15800, project_type: 'modpack' },
    { project_id: 'preview-technic-explore', title: 'Technic Preview — Explore', author: 'Technic', description: 'Демонстрационный результат предпросмотра; реальные данные доступны в desktop-версии.', downloads: 9200, project_type: 'modpack' },
  ],
}

export function demoModpacksForCatalog(catalog: ResourceCatalog): ModResult[] | null {
  if (catalog.kind !== 'ftb' && catalog.kind !== 'technic') return null
  return demoResults[catalog.kind].map((item) => ({ ...item, source: catalog.kind, catalog_id: catalog.id }))
}

export function searchDemoModpacks(
  results: ModResult[],
  query: string,
  page: number,
  limit: number,
): { results: ModResult[]; total: number; page: number; page_size: number; has_more: boolean } {
  const needle = query.trim().toLocaleLowerCase()
  const filtered = needle
    ? results.filter((item) => `${item.title || item.name || ''} ${item.description || ''}`.toLocaleLowerCase().includes(needle))
    : results
  const safePage = Number.isFinite(page) ? Math.max(0, Math.floor(page)) : 0
  const safeLimit = Number.isFinite(limit) ? Math.max(1, Math.min(30, Math.floor(limit))) : 20
  const pageResults = filtered.slice(safePage * safeLimit, (safePage + 1) * safeLimit)
  return {
    results: pageResults,
    total: filtered.length,
    page: safePage,
    page_size: pageResults.length,
    has_more: (safePage + 1) * safeLimit < filtered.length,
  }
}
