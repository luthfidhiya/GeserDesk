import { describe, expect, it } from 'vitest'
import {
  BLANK_HIDE_MS,
  HIDE_AFTER_CANCELLED_MS,
  HIDE_AFTER_DONE_MS,
  HIDE_AFTER_FAILED_MS,
  STALL_AFTER_MS,
  formatBytes,
  heading,
  hideDelay,
  isStalled,
  percent,
} from './transfer'
import type { TransferProgress, TransferState } from '@/types'

const report = (state: TransferState): TransferProgress => ({
  seq: 1,
  peer: 'windows-pc',
  file: 'a.txt',
  file_index: 0,
  file_count: 1,
  bytes_done: 5,
  bytes_total: 10,
  state,
})

describe('hideDelay', () => {
  it('never hides a pull that is still running', () => {
    expect(hideDelay({ kind: 'running' })).toBeNull()
  })

  it('hides each ending after its own time, a failure last so its message stays readable', () => {
    expect(hideDelay({ kind: 'done' })).toBe(HIDE_AFTER_DONE_MS)
    expect(hideDelay({ kind: 'cancelled' })).toBe(HIDE_AFTER_CANCELLED_MS)
    expect(hideDelay({ kind: 'failed', detail: 'putus' })).toBe(HIDE_AFTER_FAILED_MS)
    expect(HIDE_AFTER_FAILED_MS).toBeGreaterThan(HIDE_AFTER_DONE_MS)
  })
})

describe('isStalled', () => {
  it('is false until the silence has lasted the whole limit', () => {
    expect(isStalled(1000, 1000 + STALL_AFTER_MS - 1)).toBe(false)
    expect(isStalled(1000, 1000 + STALL_AFTER_MS)).toBe(true)
  })

  it('waits longer than the 30 s the Rust side gives a chunk, so its own report comes first', () => {
    expect(STALL_AFTER_MS).toBeGreaterThan(30_000)
  })
})

describe('heading', () => {
  it('names the peer while receiving', () => {
    expect(heading(report({ kind: 'running' }), false, false)).toBe('Menerima dari windows-pc')
  })

  it('says it is cancelling between the click and the answer', () => {
    expect(heading(report({ kind: 'running' }), false, true)).toBe('Membatalkan…')
  })

  it('says nothing has been heard once the pull looks stuck, whether or not Cancel was pressed', () => {
    expect(heading(report({ kind: 'running' }), true, false)).toBe('Tidak ada kabar dari windows-pc')
    expect(heading(report({ kind: 'running' }), true, true)).toBe('Tidak ada kabar dari windows-pc')
  })

  it('shows how a pull ended, whatever was pending before', () => {
    expect(heading(report({ kind: 'done' }), true, true)).toBe('Selesai')
    expect(heading(report({ kind: 'cancelled' }), true, true)).toBe('Dibatalkan')
    expect(heading(report({ kind: 'failed', detail: 'x' }), true, true)).toBe('Gagal')
  })
})

describe('percent and formatBytes', () => {
  it('rounds, caps at 100 and survives an empty total', () => {
    expect(percent(5, 10)).toBe(50)
    expect(percent(11, 10)).toBe(100)
    expect(percent(0, 0)).toBe(0)
  })

  it('picks a unit', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(2048)).toBe('2 KB')
    expect(formatBytes(3 * 1024 * 1024)).toBe('3.0 MB')
  })
})

describe('the timers fit together', () => {
  it('lets a window that never heard anything go before it would look stuck', () => {
    expect(BLANK_HIDE_MS).toBeGreaterThan(0)
    expect(BLANK_HIDE_MS).toBeLessThan(STALL_AFTER_MS)
  })
})
