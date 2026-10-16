import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { useLang, useT } from '@/lib/i18n'
import { errorText, ipc } from '@/lib/ipc'
import { formatBytes } from '@/lib/transfer'
import type { ReceivedCopy, SharedCopy } from '@/types'

const POLL_MS = 3000

/** A button that acts on the second click within a few seconds, saying so after the first. */
function TwoClick({ label, sure, onConfirm, disabled }: { label: string; sure: string; onConfirm: () => void; disabled?: boolean }) {
  const [armed, setArmed] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(timer.current), [])
  return (
    <Button
      variant={armed ? 'destructive' : 'outline'}
      size="sm"
      disabled={disabled}
      onClick={() => {
        if (!armed) {
          setArmed(true)
          timer.current = window.setTimeout(() => setArmed(false), 4000)
          return
        }
        window.clearTimeout(timer.current)
        setArmed(false)
        onConfirm()
      }}
    >
      {armed ? sure : label}
    </Button>
  )
}

/** The copies of files that went through this machine: received (on the disk) and offered (this session). */
export default function ClipboardWindow() {
  const { settings, update } = useSettings()
  const session = useSession()
  useTheme(settings?.theme)
  const t = useT()
  const lang = useLang()
  const [received, setReceived] = useState<ReceivedCopy[]>([])
  const [shared, setShared] = useState<SharedCopy[]>([])
  const [problem, setProblem] = useState<string | null>(null)

  const load = async () => {
    try {
      const [r, s] = await Promise.all([ipc.receivedFiles(), ipc.sharedFiles()])
      setReceived(r)
      setShared(s)
    } catch (e) {
      setProblem(errorText(e))
    }
  }

  useEffect(() => {
    void load()
    const id = window.setInterval(() => void load(), POLL_MS)
    return () => window.clearInterval(id)
  }, [])

  const act = async (action: () => Promise<void>) => {
    try {
      await action()
      setProblem(null)
    } catch (e) {
      setProblem(errorText(e))
    }
    await load()
  }

  const when = (secs: number) =>
    new Date(secs * 1000).toLocaleString(lang === 'en' ? 'en-GB' : 'id-ID', { dateStyle: 'medium', timeStyle: 'short' })
  const what = (names: string[], files: number) => {
    const first = names[0] ?? t('(kosong)', '(empty)')
    const more = names.length > 1 ? t(` + ${names.length - 1} lainnya`, ` + ${names.length - 1} more`) : ''
    return { title: names.join(', '), text: first + more, files: t(`${files} berkas`, `${files} file${files === 1 ? '' : 's'}`) }
  }
  const total = received.reduce((sum, r) => sum + r.bytes, 0)
  const sharedTotal = shared.reduce((sum, s) => sum + s.bytes, 0)

  return (
    <div className="flex h-full flex-col gap-5 overflow-y-auto px-5 py-4">
      <section className="space-y-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">{t('Diterima', 'Received')}</h2>
          <span className="text-xs text-muted-foreground">
            {t(`${received.length} salinan · ${formatBytes(total)}`, `${received.length} cop${received.length === 1 ? 'y' : 'ies'} · ${formatBytes(total)}`)}
          </span>
          <span className="ml-auto">
            <TwoClick
              label={t('Hapus semua', 'Remove all')}
              sure={t('Yakin hapus semua?', 'Remove them all?')}
              disabled={received.length === 0}
              onConfirm={() => void act(() => ipc.removeAllReceived())}
            />
          </span>
        </div>
        <p className="text-[11px] text-muted-foreground">
          {t(
            'Berkas yang ditarik dari komputer lain, disimpan di folder sementara. Menghapusnya membuat paste berkas itu tidak bisa lagi.',
            'Files pulled from other computers, kept in the temporary folder. Removing them means they can no longer be pasted.',
          )}
        </p>
        {received.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t('Tidak ada.', 'None.')}</p>
        ) : (
          <div className="divide-y divide-border/40">
            {received.map((r, i) => {
              const w = what(r.names, r.files)
              return (
                <div key={r.path} className="flex items-center gap-3 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px]" title={w.title}>
                      {w.text}
                      {i === 0 && (
                        <span className="ml-2 text-[11px] text-muted-foreground">{t('(terbaru, mungkin masih di clipboard)', '(newest, may still be on the clipboard)')}</span>
                      )}
                    </div>
                    <div className="text-[11px] text-muted-foreground">
                      {r.peer && t(`dari ${r.peer} · `, `from ${r.peer} · `)}
                      {when(r.time)} · {w.files} · {formatBytes(r.bytes)}
                    </div>
                  </div>
                  <Button variant="ghost" size="sm" onClick={() => void act(() => ipc.openReceived(r.path))}>
                    {t('Buka folder', 'Open folder')}
                  </Button>
                  {i === 0 ? (
                    <TwoClick label={t('Hapus', 'Remove')} sure={t('Yakin?', 'Sure?')} onConfirm={() => void act(() => ipc.removeReceived(r.path))} />
                  ) : (
                    <Button variant="outline" size="sm" onClick={() => void act(() => ipc.removeReceived(r.path))}>
                      {t('Hapus', 'Remove')}
                    </Button>
                  )}
                </div>
              )
            })}
          </div>
        )}
        {settings && (
          <div className="flex items-center gap-2 pt-1 text-xs text-muted-foreground">
            <span>{t('Hapus otomatis saat Mulai jika lebih dari', 'Remove automatically at Start when older than')}</span>
            <Input
              type="number"
              min={0}
              className="h-7 w-16 text-xs"
              value={settings.clipboard_keep_days}
              onChange={(e) => {
                const n = parseInt(e.target.value, 10)
                if (Number.isFinite(n) && n >= 0) update((s) => ({ ...s, clipboard_keep_days: n }))
              }}
            />
            <span>{t('hari (0 = tidak pernah)', 'days (0 = never)')}</span>
          </div>
        )}
      </section>

      <section className="space-y-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">{t('Dibagikan', 'Shared')}</h2>
          <span className="text-xs text-muted-foreground">
            {t(`${shared.length} salinan · ${formatBytes(sharedTotal)}`, `${shared.length} cop${shared.length === 1 ? 'y' : 'ies'} · ${formatBytes(sharedTotal)}`)}
          </span>
          <span className="ml-auto">
            <Button variant="outline" size="sm" disabled={shared.length === 0} onClick={() => void act(() => ipc.forgetShared(null))}>
              {t('Kosongkan riwayat', 'Clear the history')}
            </Button>
          </span>
        </div>
        <p className="text-[11px] text-muted-foreground">
          {t(
            'Berkas yang disalin di komputer ini dan ditawarkan ke komputer lain selama sesi ini. Tidak ada salinan yang dibuat: menghapus dari daftar tidak menyentuh berkas aslinya.',
            'Files copied on this computer and offered to the others during this session. No copy is made: removing one from the list does not touch the original files.',
          )}
        </p>
        {!session.running ? (
          <p className="text-sm text-muted-foreground">{t('Tidak ada sesi yang berjalan.', 'No session is running.')}</p>
        ) : shared.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t('Tidak ada.', 'None.')}</p>
        ) : (
          <div className="divide-y divide-border/40">
            {shared.map((s) => {
              const w = what(s.names, s.files)
              return (
                <div key={s.id} className="flex items-center gap-3 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px]" title={w.title}>{w.text}</div>
                    <div className="text-[11px] text-muted-foreground">
                      {when(s.time)} · {w.files} · {formatBytes(s.bytes)}
                    </div>
                  </div>
                  <Button variant="ghost" size="sm" title={t('Hapus dari daftar', 'Remove from the list')} onClick={() => void act(() => ipc.forgetShared(s.id))}>
                    ×
                  </Button>
                </div>
              )
            })}
          </div>
        )}
      </section>

      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </div>
  )
}
