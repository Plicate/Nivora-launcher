export interface ModCatalogSearchContextInput {
  view: 'mods' | 'modpacks'
  selectedInstanceName?: string
  selectedInstanceVersion?: string
  selectedInstanceLoader?: string
  selectedVersion: string
  selectedLoader: string
}

export interface ModCatalogSearchContext {
  instance_name: string
  game_version: string
  loader: string
}

export function modCatalogSearchContext(input: ModCatalogSearchContextInput): ModCatalogSearchContext {
  if (input.view === 'modpacks') {
    return {
      instance_name: '',
      game_version: input.selectedVersion,
      loader: input.selectedLoader,
    }
  }

  return {
    instance_name: input.selectedInstanceName || '',
    game_version: input.selectedInstanceVersion || input.selectedVersion,
    loader: input.selectedInstanceLoader || input.selectedLoader,
  }
}
