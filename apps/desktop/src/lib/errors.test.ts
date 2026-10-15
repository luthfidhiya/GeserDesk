import { describe, expect, it } from 'vitest'
import { localizeError } from '@/lib/errors'

describe('localizeError', () => {
  it('puts a known message into the language, keeping what is around it', () => {
    expect(localizeError('Sudah berjalan. Hentikan dulu.', 'en')).toBe('Already running. Stop it first.')
    expect(localizeError('ctrl+banana: unknown key "banana"', 'id')).toBe('ctrl+banana: tombol "banana" tidak dikenal')
    expect(localizeError('layar a dan b saling tumpang tindih', 'en')).toBe('screens a and b overlap')
  })

  it('leaves a message it does not know as it is', () => {
    expect(localizeError('something odd happened', 'id')).toBe('something odd happened')
    expect(localizeError('Sudah berjalan. Hentikan dulu.', 'id')).toBe('Sudah berjalan. Hentikan dulu.')
  })
})
