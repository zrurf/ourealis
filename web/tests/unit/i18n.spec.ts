import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from '@playwright/test'
import en from '../../src/locales/en'
import zhCN from '../../src/locales/zh-CN'

/** Every leaf of a catalog as `[dotted key, value]`, in declaration order. */
function leaves(
  catalog: unknown,
  prefix = '',
  out: Array<[string, unknown]> = [],
): Array<[string, unknown]> {
  if (catalog !== null && typeof catalog === 'object' && !Array.isArray(catalog)) {
    for (const [key, child] of Object.entries(catalog)) {
      leaves(child, prefix === '' ? key : `${prefix}.${key}`, out)
    }
    return out
  }
  out.push([prefix, catalog])
  return out
}

/** Dotted paths of every leaf of a catalog. */
function leafKeys(catalog: unknown): string[] {
  return leaves(catalog).map(([key]) => key)
}

/** Root of the front-end sources, from this file's own location. */
const SOURCE_ROOT = fileURLToPath(new URL('../../src', import.meta.url))

/** Extensions the key scan reads: components and modules both call `t`. */
const SCANNED_EXTENSIONS = ['.ts', '.vue']

/**
 * Matches a translation call with a literal key: `t('…')`, `t("…")`, `$t('…')`
 * and the template-literal spelling, with or without further arguments.
 *
 * The character before the callee must not be part of an identifier, so `format(`
 * and `test(` are not calls; requiring a closing quote followed by `)` or `,` is
 * what excludes a key built by concatenation, which no scan can resolve.
 */
const CALL_SITE = /(?:^|[^\w$])(?:\$t|t)\(\s*(['"`])([^'"`]*?)\1\s*[),]/g

/** Every source file under `src` the scan reads, recursively. */
function sourceFiles(directory = SOURCE_ROOT): string[] {
  const found: string[] = []
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      found.push(...sourceFiles(path))
    } else if (SCANNED_EXTENSIONS.some((extension) => entry.name.endsWith(extension))) {
      found.push(path)
    }
  }
  return found
}

/** Literal keys one source file asks for, in file order. */
function referencedKeys(source: string): string[] {
  const keys: string[] = []
  for (const match of source.matchAll(CALL_SITE)) {
    const key = match[2] ?? ''
    // A template literal with an interpolation is built at run time, so there is no
    // key to check; a template literal without one is a literal like any other.
    if (key !== '' && !key.includes('${')) {
      keys.push(key)
    }
  }
  return keys
}

test.describe('message catalogs', () => {
  test('english and chinese catalogs expose the same key set', () => {
    const english = leafKeys(en).toSorted()
    const chinese = leafKeys(zhCN).toSorted()

    expect(english.length).toBeGreaterThan(30)
    expect(chinese).toEqual(english)
  })

  test('english values are non-empty strings', () => {
    for (const [key, value] of leaves(en)) {
      expect(typeof value, key).toBe('string')
      expect(String(value).trim(), key).not.toBe('')
    }
  })

  test('the two catalogs actually differ', () => {
    const english = new Map(leaves(en))
    const translated = leaves(zhCN).filter(([key, value]) => english.get(key) !== value)

    expect(translated.length).toBeGreaterThan(0)
  })

  test('every key the sources ask for resolves in both catalogs', () => {
    // A key that exists in neither catalog renders as the raw key in the interface,
    // which is why this check follows the call sites rather than the catalogs: the
    // catalogs can only disagree with each other, not with the components.
    const english = new Set(leafKeys(en))
    const chinese = new Set(leafKeys(zhCN))
    const files = sourceFiles()
    expect(files.length, 'the scan must find the front-end sources').toBeGreaterThan(10)

    const unresolved: string[] = []
    let checked = 0
    for (const file of files) {
      for (const key of referencedKeys(readFileSync(file, 'utf8'))) {
        checked += 1
        const where = relative(SOURCE_ROOT, file).replaceAll('\\', '/')
        if (!english.has(key)) {
          unresolved.push(`${key} used in ${where} is not in the English catalog`)
        }
        if (!chinese.has(key)) {
          unresolved.push(`${key} used in ${where} is not in the Chinese catalog`)
        }
      }
    }

    expect(checked, 'the scan must find translation call sites').toBeGreaterThan(100)
    expect(unresolved, `unresolved message keys:\n${unresolved.join('\n')}`).toEqual([])
  })
})
