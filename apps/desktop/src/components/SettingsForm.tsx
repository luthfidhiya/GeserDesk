import { open } from '@tauri-apps/plugin-dialog'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { errorText, ipc } from '@/lib/ipc'
import type { Settings } from '@/types'

interface Props {
  settings: Settings
  disabled: boolean
  onChange: (change: (s: Settings) => Settings) => void
  onProblem: (text: string | null) => void
}

export function ClientForm({ settings, disabled, onChange }: Props) {
  const c = settings.client
  const set = (patch: Partial<Settings['client']>) =>
    onChange((s) => ({ ...s, client: { ...s.client, ...patch } }))
  return (
    <div className="grid grid-cols-[10rem_1fr] items-center gap-x-3 gap-y-3">
      <Label htmlFor="srv">Alamat server</Label>
      <Input
        id="srv"
        disabled={disabled}
        value={c.server}
        placeholder="contoh 192.168.100.10 atau 192.168.100.10:24810"
        onChange={(e) => set({ server: e.target.value })}
      />
      <Label htmlFor="nm">Nama layar ini</Label>
      <Input id="nm" disabled={disabled} value={c.name} onChange={(e) => set({ name: e.target.value })} />
      <span />
      <div className="flex items-center gap-2">
        <Checkbox
          id="logonly"
          disabled={disabled}
          checked={c.log_only}
          onCheckedChange={(v) => set({ log_only: v === true })}
        />
        <Label htmlFor="logonly">
          Hanya catat input di log (untuk uji coba, tanpa menggerakkan mouse)
        </Label>
      </div>
    </div>
  )
}

export function ServerForm({ settings, disabled, onChange, onProblem }: Props) {
  const path = settings.server.config
  const [exists, setExists] = useState(true)
  const [defaultPath, setDefaultPath] = useState<string | null>(null)

  useEffect(() => {
    void ipc.defaultConfigPath().then(setDefaultPath)
  }, [])

  useEffect(() => {
    let current = true
    void ipc.configExists(path.trim()).then((e) => current && setExists(e))
    return () => {
      current = false
    }
  }, [path])

  const set = (config: string) => onChange((s) => ({ ...s, server: { ...s.server, config } }))

  const browse = async () => {
    const picked = await open({ multiple: false, filters: [{ name: 'TOML', extensions: ['toml'] }] })
    if (typeof picked === 'string') set(picked)
  }

  const create = async () => {
    try {
      await ipc.createDefaultConfig(path.trim())
      onProblem(null)
      setExists(true)
    } catch (e) {
      onProblem(errorText(e))
    }
  }

  // The default file is made by Start, so it needs no setting. The field is for a
  // different file, and stays shut unless that is in use or the file is missing.
  const custom = defaultPath !== null && path.trim() !== defaultPath
  const needsAttention = custom && !exists

  return (
    <details className="group rounded-lg border p-3" open={needsAttention || undefined}>
      <summary className="cursor-pointer text-sm text-muted-foreground">
        Lanjutan{custom ? ': memakai berkas konfigurasi lain' : ''}
      </summary>
      <div className="mt-3 space-y-3">
        <div className="grid grid-cols-[10rem_1fr_auto] items-center gap-x-3">
          <Label htmlFor="cfg">Berkas konfigurasi</Label>
          <Input id="cfg" disabled={disabled} value={path} onChange={(e) => set(e.target.value)} />
          <Button variant="outline" disabled={disabled} onClick={() => void browse()}>
            Pilih…
          </Button>
        </div>
        {custom && (
          <Button variant="link" size="sm" className="h-auto p-0 text-xs" disabled={disabled} onClick={() => defaultPath && set(defaultPath)}>
            Kembali ke berkas bawaan
          </Button>
        )}
        {needsAttention && !disabled && (
          <Button variant="secondary" onClick={() => void create()}>
            Buat berkas konfigurasi awal
          </Button>
        )}
      </div>
    </details>
  )
}
