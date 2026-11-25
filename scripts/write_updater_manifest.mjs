import { writeFileSync } from 'node:fs'

const [outputPath, version, releaseRepo, releaseTag, assetName, pubDate, signature] = process.argv.slice(2)
if (![outputPath, version, releaseRepo, releaseTag, assetName, pubDate].every(Boolean)) {
  throw new Error('Expected output path, version, repository, release tag, asset name, and publication date')
}

const repoParts = releaseRepo.split('/')
if (repoParts.length !== 2 || repoParts.some((part) => !part)) {
  throw new Error('Release repository must be in owner/name form')
}

const encodePathSegment = (value) => encodeURIComponent(value)
const assetUrl = [
  'https://github.com',
  ...repoParts.map(encodePathSegment),
  'releases/download',
  encodePathSegment(releaseTag),
  encodePathSegment(assetName),
].join('/')
// подпись опциональна: вендоренный апдейтер пропускает проверку при пустом pubkey
const targetEntry = signature ? { signature, url: assetUrl } : { url: assetUrl }
const platforms = Object.fromEntries(['windows-x86_64', 'windows-x86_64-nsis'].map((target) => [target, targetEntry]))
const manifest = {
  version,
  notes: `Nivora Launcher ${version}`,
  pub_date: pubDate,
  platforms,
}
const serialized = `${JSON.stringify(manifest, null, 2)}\n`
const parsed = JSON.parse(serialized)
for (const target of Object.keys(platforms)) {
  const entry = parsed.platforms[target]
  const signatureOk = signature ? entry?.signature === signature : entry?.signature === undefined
  if (entry?.url !== assetUrl || !signatureOk) {
    throw new Error(`Invalid updater manifest for ${target}`)
  }
}
writeFileSync(outputPath, serialized, 'utf8')
