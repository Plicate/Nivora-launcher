import { writeFileSync } from 'node:fs'

const [outputPath, version, releaseRepo, releaseTag, assetName, signature, pubDate] = process.argv.slice(2)
if (![outputPath, version, releaseRepo, releaseTag, assetName, signature, pubDate].every(Boolean)) {
  throw new Error('Expected output path, version, repository, release tag, asset name, signature, and publication date')
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
const platforms = Object.fromEntries(['windows-x86_64', 'windows-x86_64-nsis'].map((target) => [target, {
  signature,
  url: assetUrl,
}]))
const manifest = {
  version,
  notes: `Nivora Launcher ${version}`,
  pub_date: pubDate,
  platforms,
}
const serialized = `${JSON.stringify(manifest, null, 2)}\n`
const parsed = JSON.parse(serialized)
for (const target of Object.keys(platforms)) {
  if (parsed.platforms[target]?.signature !== signature || parsed.platforms[target]?.url !== assetUrl) {
    throw new Error(`Invalid updater manifest for ${target}`)
  }
}
writeFileSync(outputPath, serialized, 'utf8')
